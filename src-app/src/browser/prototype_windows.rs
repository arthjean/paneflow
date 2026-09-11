use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::time::{Duration, Instant};

use clap::Parser;
use gpui::{
    App, Bounds, Context, Entity, FocusHandle, KeyDownEvent, KeyUpEvent, Modifiers, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Render, ScrollDelta, ScrollWheelEvent, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, size,
};
use paneflow_browser_protocol::{
    BrowserError, BrowserId, BrowserSession, Command, Event, InputEvent, KeyKind,
    MODIFIER_LEFT_MOUSE, MouseButton, Owner, ProfileId, SessionId, SessionState, WorkspaceId,
};
use serde_json::{Value, json};

use super::input::{button_modifier, consume_key_down, key_events, modifiers, mouse_button};
use super::page::{Geometry, LivePage, PageConfig, PageSignal};
use super::supervisor::RuntimeCheck;
use super::{BrowserRuntime, origin_of};

const MONITOR_PRIMARY_FLAG: u32 = 1;

#[derive(Clone)]
struct MonitorBounds {
    handle: windows_sys::Win32::Graphics::Gdi::HMONITOR,
    rect: windows_sys::Win32::Foundation::RECT,
    device: String,
    primary: bool,
    scale_percent: u32,
}

impl MonitorBounds {
    fn report(&self) -> Value {
        json!({
            "device": self.device,
            "primary": self.primary,
            "scale_percent": self.scale_percent,
            "bounds": {
                "left": self.rect.left,
                "top": self.rect.top,
                "right": self.rect.right,
                "bottom": self.rect.bottom
            }
        })
    }
}

fn monitor_scale_percent(monitor: windows_sys::Win32::Graphics::Gdi::HMONITOR) -> u32 {
    let mut horizontal = 96u32;
    let mut vertical = 96u32;
    let status = unsafe {
        windows_sys::Win32::UI::HiDpi::GetDpiForMonitor(
            monitor,
            windows_sys::Win32::UI::HiDpi::MDT_EFFECTIVE_DPI,
            &mut horizontal,
            &mut vertical,
        )
    };
    if status != 0 {
        return 100;
    }
    (horizontal * 100).div_ceil(96)
}

unsafe extern "system" fn collect_monitor_bounds(
    monitor: windows_sys::Win32::Graphics::Gdi::HMONITOR,
    _dc: windows_sys::Win32::Graphics::Gdi::HDC,
    _clip: *mut windows_sys::Win32::Foundation::RECT,
    data: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::BOOL {
    let monitors = unsafe { &mut *(data as *mut Vec<MonitorBounds>) };
    let mut info: windows_sys::Win32::Graphics::Gdi::MONITORINFOEXW = unsafe { std::mem::zeroed() };
    info.monitorInfo.cbSize =
        std::mem::size_of::<windows_sys::Win32::Graphics::Gdi::MONITORINFOEXW>() as u32;
    if unsafe {
        windows_sys::Win32::Graphics::Gdi::GetMonitorInfoW(
            monitor,
            &mut info as *mut _ as *mut windows_sys::Win32::Graphics::Gdi::MONITORINFO,
        )
    } != 0
    {
        let length = info
            .szDevice
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(info.szDevice.len());
        monitors.push(MonitorBounds {
            handle: monitor,
            rect: info.monitorInfo.rcMonitor,
            device: String::from_utf16_lossy(&info.szDevice[..length]),
            primary: info.monitorInfo.dwFlags & MONITOR_PRIMARY_FLAG != 0,
            scale_percent: monitor_scale_percent(monitor),
        });
    }
    1
}

fn native_monitors() -> Vec<MonitorBounds> {
    let mut monitors: Vec<MonitorBounds> = Vec::new();
    let result = unsafe {
        windows_sys::Win32::Graphics::Gdi::EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(collect_monitor_bounds),
            &mut monitors as *mut Vec<MonitorBounds> as isize,
        )
    };
    if result == 0 {
        return Vec::new();
    }
    monitors
}

fn window_monitor(window: &Window) -> Result<MonitorBounds, String> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromWindow};

    let handle = HasWindowHandle::window_handle(window).map_err(|error| error.to_string())?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return Err("browser prototype did not expose a Win32 window handle".to_string());
    };
    let hwnd = handle.hwnd.get() as windows_sys::Win32::Foundation::HWND;
    let current = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    native_monitors()
        .into_iter()
        .find(|monitor| monitor.handle == current)
        .ok_or_else(|| "the browser prototype window is not on an enumerated monitor".to_string())
}

fn registry_value(name: &str) -> Option<Value> {
    use windows_sys::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
    };

    let key: Vec<u16> = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let value: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut number = 0u32;
    let mut length = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut number as *mut u32 as *mut std::ffi::c_void,
            &mut length,
        )
    };
    if status == 0 {
        return Some(json!(number));
    }
    let mut text = [0u16; 256];
    let mut length = std::mem::size_of_val(&text) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            text.as_mut_ptr() as *mut std::ffi::c_void,
            &mut length,
        )
    };
    if status != 0 {
        return None;
    }
    let units = (length as usize / std::mem::size_of::<u16>()).min(text.len());
    let units = text[..units]
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(units);
    Some(json!(String::from_utf16_lossy(&text[..units])))
}

fn machine_report() -> Value {
    let build = registry_value("CurrentBuild")
        .and_then(|value| value.as_str().and_then(|text| text.parse::<u32>().ok()))
        .unwrap_or(0);
    json!({
        "current_build": build,
        "update_build_revision": registry_value("UBR"),
        "display_version": registry_value("DisplayVersion"),
        "release_id": registry_value("ReleaseId"),
        "edition_id": registry_value("EditionID"),
        "registry_product_name": registry_value("ProductName"),
        "registry_product_name_note": "ProductName keeps the Windows 10 string on Windows 11; the build number carries the identity",
        "installation_type": registry_value("InstallationType"),
        "build_lab": registry_value("BuildLabEx"),
        "windows_11": build >= 22000,
        "product_line": if build >= 22000 { "Windows 11" } else { "Windows 10" },
        "pointer_width": usize::BITS,
        "monitors": native_monitors().iter().map(MonitorBounds::report).collect::<Vec<_>>(),
    })
}

fn move_window_to_next_monitor(window: &Window) -> Result<(MonitorBounds, MonitorBounds), String> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromWindow};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowRect, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos,
    };

    let handle = HasWindowHandle::window_handle(window).map_err(|error| error.to_string())?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return Err("browser prototype did not expose a Win32 window handle".to_string());
    };
    let hwnd = handle.hwnd.get() as windows_sys::Win32::Foundation::HWND;
    let current_handle = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    if current_handle.is_null() {
        return Err("Windows returned no monitor for the browser prototype window".to_string());
    }

    let monitors = native_monitors();
    if monitors.len() < 2 {
        return Err("DPI qualification requires at least two native monitors".to_string());
    }
    let current_index = monitors
        .iter()
        .position(|monitor| monitor.handle == current_handle)
        .ok_or_else(|| "current monitor was not returned by EnumDisplayMonitors".to_string())?;
    let target = monitors[(current_index + 1) % monitors.len()].clone();
    let current = monitors[current_index].clone();
    let mut window_rect = windows_sys::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if unsafe { GetWindowRect(hwnd, &mut window_rect) } == 0 {
        return Err("GetWindowRect failed for the browser prototype window".to_string());
    }
    let width = window_rect.right - window_rect.left;
    let height = window_rect.bottom - window_rect.top;
    let moved = unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            target.rect.left + 64,
            target.rect.top + 64,
            width,
            height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
    if moved == 0 {
        return Err("SetWindowPos failed while moving the browser prototype window".to_string());
    }
    Ok((current, target))
}

const TICK: Duration = Duration::from_millis(16);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(45);
const RUN_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Parser, Debug)]
#[command(
    name = "paneflow browser-prototype",
    about = "Present a page through the Windows DirectX external-surface path"
)]
struct Options {
    #[arg(long, help = "Page to open: an absolute http or https URL")]
    url: Option<String>,
    #[arg(long, default_value = "cef", help = "Windows supports the cef source")]
    source: String,
    #[arg(
        long,
        help = "Comma-separated steps: input,wheel,drag,resize,scale,dpi,ime,cancel,agent"
    )]
    scenario: Option<String>,
    #[arg(
        long,
        help = "Second absolute http or https URL the agent step navigates to; required by that step"
    )]
    agent_url: Option<String>,
    #[arg(
        long,
        help = "Comma-separated display scales in percent for the dpi step; the step refuses a primary monitor and restores the previous scale"
    )]
    dpi_transitions: Option<String>,
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
    #[arg(
        long,
        default_value_t = false,
        help = "Reserved for cross-platform CLI parity"
    )]
    x11: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Input,
    Wheel,
    Drag,
    Resize,
    Scale,
    Dpi,
    Ime,
    Cancel,
    Agent,
}

impl Step {
    fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "input" => Some(Self::Input),
            "wheel" => Some(Self::Wheel),
            "drag" => Some(Self::Drag),
            "resize" => Some(Self::Resize),
            "scale" => Some(Self::Scale),
            "dpi" => Some(Self::Dpi),
            "ime" => Some(Self::Ime),
            "cancel" => Some(Self::Cancel),
            "agent" => Some(Self::Agent),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Wheel => "wheel",
            Self::Drag => "drag",
            Self::Resize => "resize",
            Self::Scale => "scale",
            Self::Dpi => "dpi",
            Self::Ime => "ime",
            Self::Cancel => "cancel",
            Self::Agent => "agent",
        }
    }
}

#[derive(Clone)]
struct Logger {
    sender: Option<SyncSender<Value>>,
    origin: Arc<Instant>,
}

impl Logger {
    fn new(path: Option<&PathBuf>) -> Result<Self, String> {
        let origin = Arc::new(Instant::now());
        let Some(path) = path else {
            return Ok(Self {
                sender: None,
                origin,
            });
        };
        if !path.is_absolute() {
            return Err("--log must be an absolute path".to_string());
        }
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
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
            origin,
        })
    }

    fn emit(&self, event: &str, mut fields: Value) {
        fields["event"] = json!(event);
        fields["at_ns"] = json!(self.origin.elapsed().as_nanos() as u64);
        if let Some(sender) = &self.sender {
            let _ = sender.try_send(fields);
        } else if let Ok(text) = serde_json::to_string(&fields) {
            eprintln!("{text}");
        }
    }
}

enum DpiPhase {
    Apply,
    Settle { wanted: u32, deadline: Instant },
    Sample { wanted: u32, at: Instant },
    Restore { deadline: Instant },
}

struct DpiRun {
    monitor: MonitorBounds,
    source: super::display_scale::DisplaySource,
    range: super::display_scale::ScaleRange,
    baseline_percent: u32,
    pending: VecDeque<u32>,
    phase: DpiPhase,
    scale: Option<super::display_scale::ScaleOverride>,
}

#[derive(Clone, Copy)]
enum ImeResolution {
    Commit(&'static str),
    Cancel,
    Finish,
}

#[derive(Clone, Copy)]
struct ImeCase {
    name: &'static str,
    composition: &'static str,
    replaces_composition: bool,
    resolution: ImeResolution,
}

const IME_CORPUS: [ImeCase; 6] = [
    ImeCase {
        name: "ascii",
        composition: "paneflow",
        replaces_composition: false,
        resolution: ImeResolution::Commit("paneflow"),
    },
    ImeCase {
        name: "accented",
        composition: "éàü",
        replaces_composition: false,
        resolution: ImeResolution::Commit("éàü"),
    },
    ImeCase {
        name: "japanese_candidate",
        composition: "にほんご",
        replaces_composition: true,
        resolution: ImeResolution::Commit("日本語"),
    },
    ImeCase {
        name: "astral",
        composition: "😀",
        replaces_composition: false,
        resolution: ImeResolution::Commit("😀"),
    },
    ImeCase {
        name: "cancelled",
        composition: "annulation",
        replaces_composition: false,
        resolution: ImeResolution::Cancel,
    },
    ImeCase {
        name: "finished",
        composition: "fin",
        replaces_composition: false,
        resolution: ImeResolution::Finish,
    },
];

enum ImePhase {
    Focus { at: Instant },
    Compose,
    Observe { case: ImeCase, at: Instant },
    Settle { at: Instant },
}

struct ImeRun {
    cases: VecDeque<ImeCase>,
    phase: ImePhase,
}

enum CancelPhase {
    Focus,
    Capture,
    Stale { deadline: Instant },
    Viewport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AgentPhase {
    Navigate,
    AwaitCommit,
    Stale,
    AwaitStale,
    Cancel,
    Takeover,
    AwaitCancel,
}

struct AgentRun {
    phase: AgentPhase,
    operation: Option<paneflow_browser_protocol::OperationId>,
    committed_generation: Option<u64>,
    committed_url: Option<String>,
    stale_refused: bool,
    cancelled: bool,
    takeover_after: Option<u64>,
    deadline: Instant,
}

struct PrototypeView {
    options: Arc<Options>,
    log: Logger,
    exit: Arc<AtomicI32>,
    focus: FocusHandle,
    owner: Owner,
    browser: BrowserId,
    profile: ProfileId,
    page: Option<LivePage>,
    session: Option<BrowserSession>,
    ime: super::ime::ImeState,
    ime_composing: bool,
    started: Instant,
    first_frame: Option<Instant>,
    hold_until: Option<Instant>,
    last_geometry: Option<Geometry>,
    steps: VecDeque<Step>,
    step_started: Option<Instant>,
    dpi: Option<DpiRun>,
    ime_run: Option<ImeRun>,
    cancel_phase: Option<CancelPhase>,
    agent_run: Option<AgentRun>,
    text_focus_sent: bool,
    expected_failures: Vec<paneflow_browser_protocol::OperationId>,
    expected_refusals: usize,
    observed_cancellations: u64,
    created: bool,
    loaded: bool,
    frame_events: u64,
    started_command: bool,
    finished: bool,
    failure: Option<String>,
}

impl PrototypeView {
    fn new(
        options: Arc<Options>,
        log: Logger,
        exit: Arc<AtomicI32>,
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
        let owner = Owner {
            workspace: WorkspaceId::try_from("prototype".to_string())
                .unwrap_or_else(|_| unreachable!()),
            session: SessionId::try_from("browser".to_string()).unwrap_or_else(|_| unreachable!()),
        };
        let browser =
            BrowserId::try_from("prototype".to_string()).unwrap_or_else(|_| unreachable!());
        let profile =
            ProfileId::try_from("prototype".to_string()).unwrap_or_else(|_| unreachable!());
        Self {
            options,
            log,
            exit,
            focus: cx.focus_handle(),
            owner,
            browser,
            profile,
            page: None,
            session: None,
            ime: super::ime::ImeState::default(),
            ime_composing: false,
            started: Instant::now(),
            first_frame: None,
            hold_until: None,
            last_geometry: None,
            steps,
            step_started: None,
            dpi: None,
            ime_run: None,
            cancel_phase: None,
            agent_run: None,
            text_focus_sent: false,
            expected_failures: Vec::new(),
            expected_refusals: 0,
            observed_cancellations: 0,
            created: false,
            loaded: false,
            frame_events: 0,
            started_command: false,
            finished: false,
            failure: None,
        }
    }

    fn fail(&mut self, reason: String, cx: &mut Context<Self>) {
        if self.finished {
            return;
        }
        self.failure = Some(reason.clone());
        self.log.emit("fatal", json!({ "reason": reason }));
        self.finish(cx);
    }

    fn finish(&mut self, cx: &mut Context<Self>) {
        if self.finished {
            return;
        }
        self.finished = true;
        if let Some(mut run) = self.dpi.take() {
            let restored = run.scale.as_mut().map(|scale| scale.restore());
            self.log.emit(
                "dpi_restored",
                json!({
                    "monitor": run.monitor.device,
                    "scale_percent": run.baseline_percent,
                    "during_shutdown": true,
                    "error": restored.and_then(|result| result.err())
                }),
            );
        }
        let document = self.page.as_ref().and_then(LivePage::document).cloned();
        if let (Some(page), Some(document)) = (&self.page, document) {
            let _ = page.send(Command::Close { document });
        }
        if let Some(page) = self.page.take() {
            page.shutdown();
        }
        let status = if self.failure.is_some() {
            "FAILED"
        } else {
            "COMPLETED"
        };
        self.log.emit(
            "summary",
            json!({
                "status": status,
                "failure": self.failure,
                "source": self.options.source,
                "x11": self.options.x11,
                "created": self.created,
                "loaded": self.loaded,
                "frame_events": self.frame_events,
                "cancellations": self.observed_cancellations,
                "pending_expected_refusals": self.expected_refusals,
                "first_frame_after_ms": self.first_frame.map(|at| at.duration_since(self.started).as_millis()),
                "remaining_steps": self.steps.iter().map(|step| step.name()).collect::<Vec<_>>(),
            }),
        );
        self.exit
            .store(i32::from(self.failure.is_some()), Ordering::SeqCst);
        let quit = cx.background_executor().timer(Duration::from_millis(250));
        cx.spawn(async move |_this, cx| {
            quit.await;
            cx.update(|cx| cx.quit());
        })
        .detach();
    }

    fn initialize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page.is_some() || self.finished {
            return;
        }
        let Some(url) = self.options.url.clone() else {
            self.fail("--url is required for the cef source".to_string(), cx);
            return;
        };
        let origin = match origin_of(&url) {
            Ok(origin) => origin,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let host_binary = match std::env::var_os(super::HOST_ENV) {
            Some(path) => PathBuf::from(path),
            None => match std::env::current_exe() {
                Ok(path) => path
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .join("paneflow-browser-host.exe"),
                Err(error) => {
                    self.fail(format!("cannot resolve the browser host: {error}"), cx);
                    return;
                }
            },
        };
        let Some(runtime_root) = std::env::var_os(super::RUNTIME_ENV).map(PathBuf::from) else {
            self.fail(
                "PANEFLOW_CEF_ROOT is required for the cef source".to_string(),
                cx,
            );
            return;
        };
        let root = std::env::var_os("PANEFLOW_BROWSER_STAGE_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::temp_dir()
                    .join("paneflow-browser-prototype")
                    .join(std::process::id().to_string())
            });
        let profile_dir = std::env::var_os("PANEFLOW_CEF_PROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("profile"));
        if let Err(error) = fs::create_dir_all(&profile_dir) {
            self.fail(format!("cannot create the browser profile: {error}"), cx);
            return;
        }
        let runtime_check = match RuntimeCheck::configured() {
            Ok(check) => check,
            Err(error) => {
                self.fail(error, cx);
                return;
            }
        };
        let config = PageConfig {
            benchmark_id: self.browser.as_str().to_string(),
            host_binary,
            runtime_root,
            stage_root: root,
            profile_dir,
            origin,
            owner: self.owner.clone(),
            runtime_check,
        };
        match LivePage::start(window, cx, config) {
            Ok(start) => {
                self.page = Some(start.page);
                let host_events = start.host_events;
                cx.spawn(async move |this, cx| {
                    while let Ok(event) = host_events.recv().await {
                        if this
                            .update(cx, |view, cx| view.on_host_event(event, cx))
                            .is_err()
                        {
                            return;
                        }
                    }
                })
                .detach();
                let gpu_completions = start.gpu_completions;
                cx.spawn(async move |this, cx| {
                    while let Ok(completion) = gpu_completions.recv().await {
                        if this
                            .update(cx, |view, cx| {
                                if let Some(page) = &mut view.page
                                    && let Err(error) = page.on_gpu_completion(completion)
                                {
                                    view.fail(error, cx);
                                } else {
                                    cx.notify();
                                }
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                })
                .detach();
                self.log.emit(
                    "page_started",
                    json!({ "host": self.options.url, "profile": self.profile.as_str() }),
                );
                cx.notify();
            }
            Err(error) => self.fail(format!("browser page failed to start: {error}"), cx),
        }
    }

    fn on_host_event(&mut self, event: super::supervisor::HostEvent, cx: &mut Context<Self>) {
        let Some(page) = &mut self.page else {
            return;
        };
        let signals = page.on_host_event(event, cx);
        for signal in signals {
            self.on_signal(signal, cx);
        }
        cx.notify();
    }

    fn on_signal(&mut self, signal: PageSignal, cx: &mut Context<Self>) {
        match signal {
            PageSignal::Ready => {
                if self.started_command {
                    return;
                }
                let Some(url) = self.options.url.clone() else {
                    self.fail("missing browser URL".to_string(), cx);
                    return;
                };
                let Some(page) = &self.page else {
                    return;
                };
                self.started_command = true;
                if let Err(error) = page.send(Command::Create {
                    owner: self.owner.clone(),
                    browser: self.browser.clone(),
                    profile: self.profile.clone(),
                    url,
                    title: "Windows browser prototype".to_string(),
                }) {
                    self.fail(format!("host refused browser creation: {error:?}"), cx);
                }
            }
            PageSignal::Created => {
                self.created = true;
                self.log.emit("created", json!({}));
            }
            PageSignal::State(session) => {
                let dormant = session.state == SessionState::Dormant;
                self.session = Some(session.clone());
                self.log.emit(
                    "state",
                    json!({ "state": format!("{:?}", session.state), "generation": session.document.generation }),
                );
                if dormant
                    && let Some(page) = &self.page
                    && let Err(error) =
                        page.send_to_document(|document| Command::Start { document })
                {
                    self.fail(format!("host refused browser start: {error:?}"), cx);
                }
            }
            PageSignal::Loaded => {
                self.loaded = true;
                self.log.emit("loaded", json!({}));
            }
            PageSignal::Repaint => {
                self.frame_events = self.frame_events.saturating_add(1);
                self.first_frame.get_or_insert_with(Instant::now);
                self.log
                    .emit("frame", json!({ "count": self.frame_events }));
                cx.notify();
            }
            PageSignal::OperationCompleted { operation, result } => {
                if let Some(run) = &mut self.agent_run
                    && run.operation.as_ref() == Some(&operation)
                {
                    match (run.phase, &result) {
                        (AgentPhase::AwaitCommit, Ok(Event::Completed { .. })) => {
                            run.committed_generation = self
                                .session
                                .as_ref()
                                .map(|session| session.document.generation);
                            run.committed_url = self.options.agent_url.clone();
                        }
                        (AgentPhase::AwaitStale, Err(_)) => run.stale_refused = true,
                        (AgentPhase::AwaitStale, Ok(Event::Completed { .. })) => {
                            self.fail(
                                "a stale agent document reached a document commit".to_string(),
                                cx,
                            );
                            return;
                        }
                        (AgentPhase::AwaitCancel, Err(_)) => run.cancelled = true,
                        (AgentPhase::AwaitCancel, Ok(Event::Completed { .. })) => {
                            self.fail(
                                "a human takeover did not cancel the pending agent navigation"
                                    .to_string(),
                                cx,
                            );
                            return;
                        }
                        _ => {}
                    }
                    self.log.emit(
                        "agent_operation",
                        json!({ "operation": operation.as_str(), "phase": format!("{:?}", run.phase), "result": format!("{result:?}") }),
                    );
                    cx.notify();
                    return;
                }
                let expected = self
                    .expected_failures
                    .iter()
                    .position(|pending| *pending == operation)
                    .map(|index| self.expected_failures.remove(index))
                    .is_some();
                self.log.emit(
                    "operation",
                    json!({ "operation": operation.as_str(), "ok": result.is_ok(), "expected_failure": expected, "result": format!("{result:?}") }),
                );
                if let Err(error) = result
                    && !expected
                {
                    self.fail(format!("browser operation failed: {error:?}"), cx);
                }
            }
            PageSignal::Refused(error) => {
                if self.expected_refusals == 0 {
                    self.fail(format!("browser command refused: {error:?}"), cx);
                    return;
                }
                self.expected_refusals -= 1;
                self.observed_cancellations = self.observed_cancellations.saturating_add(1);
                self.log.emit(
                    "cancellation",
                    json!({ "condition": "stale_document", "error": format!("{error:?}") }),
                );
            }
            PageSignal::Lost(reason) | PageSignal::Fatal(reason) => self.fail(reason, cx),
            PageSignal::LoadFailed(reason) => {
                self.log.emit("load_failed", json!({ "reason": reason }));
            }
            PageSignal::Title(title) => self.log.emit("title", json!({ "title": title })),
            PageSignal::Address(url) => self.log.emit("address", json!({ "url": url })),
            PageSignal::ImeSelection(snapshot) => {
                self.log.emit(
                    "ime_selection",
                    json!({"start":snapshot.start,"end":snapshot.end}),
                );
                self.ime.selection_changed(snapshot);
            }
            PageSignal::ImeBounds(snapshot) => {
                self.log.emit(
                    "ime_bounds",
                    json!({"start":snapshot.start,"end":snapshot.end,"bounds":snapshot.bounds}),
                );
                self.ime.composition_bounds(snapshot);
            }
            _ => {}
        }
    }

    fn geometry(&self, window: &Window) -> Geometry {
        let viewport = window.viewport_size();
        Geometry {
            width: f32::from(viewport.width).round().max(1.0) as u32,
            height: f32::from(viewport.height).round().max(1.0) as u32,
            scale_percent: (window.scale_factor() * 100.0).round().clamp(50.0, 400.0) as u32,
        }
    }

    fn present(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let geometry = self.geometry(window);
        if self.last_geometry == Some(geometry) {
            return;
        }
        let Some(page) = &mut self.page else {
            return;
        };
        match page.present_if_needed(geometry, true) {
            Ok(true) => {
                self.last_geometry = Some(geometry);
                self.log.emit(
                    "present",
                    json!({ "width": geometry.width, "height": geometry.height, "scale_percent": geometry.scale_percent }),
                );
            }
            Ok(false) => cx.notify(),
            Err(BrowserError::Unavailable) => {}
            Err(error) => self.fail(format!("browser presentation failed: {error:?}"), cx),
        }
    }

    fn input(&mut self, input: InputEvent, cx: &mut Context<Self>) {
        let Some(page) = &self.page else {
            return;
        };
        if let Err(error) = page.send_to_document(|document| Command::Input { document, input }) {
            self.log
                .emit("input_rejected", json!({ "error": format!("{error:?}") }));
            cx.notify();
        }
    }

    fn step_input(&mut self, window: &Window, cx: &mut Context<Self>) {
        let geometry = self.geometry(window);
        let x = (geometry.width / 2) as i32;
        let y = (geometry.height / 2) as i32;
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
        for input in [
            InputEvent::Key {
                kind: KeyKind::RawDown,
                key_code: 65,
                native_key_code: 0,
                character: 0,
                unmodified_character: 0,
                modifiers: 0,
            },
            InputEvent::Key {
                kind: KeyKind::Char,
                key_code: 65,
                native_key_code: 0,
                character: b'A' as u16,
                unmodified_character: b'A' as u16,
                modifiers: 0,
            },
            InputEvent::Key {
                kind: KeyKind::Up,
                key_code: 65,
                native_key_code: 0,
                character: 0,
                unmodified_character: 0,
                modifiers: 0,
            },
        ] {
            self.input(input, cx);
        }
        self.log.emit("input", json!({ "x": x, "y": y }));
    }

    fn step_wheel(&mut self, window: &Window, cx: &mut Context<Self>) {
        let geometry = self.geometry(window);
        self.input(
            InputEvent::MouseWheel {
                x: (geometry.width / 2) as i32,
                y: (geometry.height / 2) as i32,
                delta_x: 0,
                delta_y: -120,
                modifiers: 0,
            },
            cx,
        );
        self.log.emit("wheel", json!({ "delta_y": -120 }));
    }

    fn step_drag(&mut self, window: &Window, cx: &mut Context<Self>) {
        let geometry = self.geometry(window);
        let x = (geometry.width / 3) as i32;
        let y = (geometry.height / 2) as i32;
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
            InputEvent::MouseMove {
                x: x + 96,
                y: y + 24,
                modifiers: MODIFIER_LEFT_MOUSE,
            },
            cx,
        );
        self.input(
            InputEvent::MouseButton {
                x: x + 96,
                y: y + 24,
                button: MouseButton::Left,
                down: false,
                clicks: 1,
                modifiers: 0,
            },
            cx,
        );
        self.input(InputEvent::CaptureLost, cx);
        self.log.emit("drag", json!({ "x": x, "y": y }));
    }

    fn step_resize(&mut self, window: &mut Window) {
        let scale = window.scale_factor().max(0.01);
        window.resize(size(
            px((self.options.width.saturating_add(160)) as f32 / scale),
            px((self.options.height.saturating_add(96)) as f32 / scale),
        ));
        self.log.emit(
            "resize",
            json!({ "width": self.options.width.saturating_add(160), "height": self.options.height.saturating_add(96) }),
        );
    }

    fn step_scale(&mut self, window: &Window) -> Result<(), String> {
        let before_scale_percent = (window.scale_factor() * 100.0).round() as u32;
        let (current, target) = move_window_to_next_monitor(window)?;
        let after_scale_percent = (window.scale_factor() * 100.0).round() as u32;
        self.log.emit(
            "scale",
            json!({
                "moved": true,
                "from_monitor": current.report(),
                "to_monitor": target.report(),
                "before_scale_percent": before_scale_percent,
                "after_scale_percent": after_scale_percent
            }),
        );
        Ok(())
    }

    fn ensure_text_focus(&mut self, cx: &mut Context<Self>) {
        if self.text_focus_sent {
            return;
        }
        self.text_focus_sent = true;
        self.input(InputEvent::Focus { focused: true }, cx);
        for kind in [KeyKind::RawDown, KeyKind::Up] {
            self.input(
                InputEvent::Key {
                    kind,
                    key_code: 9,
                    native_key_code: 0,
                    character: 0,
                    unmodified_character: 0,
                    modifiers: 0,
                },
                cx,
            );
        }
        self.log.emit("text_focus", json!({ "key": "tab" }));
    }

    fn compose(&mut self, text: &str, replacement: Option<[u32; 2]>, cx: &mut Context<Self>) {
        let units = text.encode_utf16().count() as u32;
        self.ime_composing = true;
        self.ime.compose(text, None);
        self.input(
            InputEvent::ImeComposition {
                text: text.to_string(),
                cursor: units,
                selection_start: Some(units),
                replacement,
            },
            cx,
        );
    }

    fn composition_report(&self, window: &Window) -> Value {
        let geometry = self.geometry(window);
        let anchor = self
            .ime
            .marked
            .as_ref()
            .map(|range| range.start)
            .unwrap_or(0);
        json!({
            "scale_percent": geometry.scale_percent,
            "viewport": { "width": geometry.width, "height": geometry.height },
            "selection": self.ime.selection.as_ref().map(|range| [range.start, range.end]),
            "candidate_rects": self.ime.bounds.len(),
            "candidate_rect": self.ime.caret_bounds(anchor),
        })
    }

    fn step_ime(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        let Some(run) = self.ime_run.as_mut() else {
            self.ime_run = Some(ImeRun {
                cases: IME_CORPUS.iter().copied().collect(),
                phase: ImePhase::Focus {
                    at: Instant::now() + Duration::from_millis(400),
                },
            });
            self.ensure_text_focus(cx);
            self.log.emit("ime_corpus_started", json!({}));
            return false;
        };
        match run.phase {
            ImePhase::Focus { at } => {
                if Instant::now() < at {
                    return false;
                }
                run.phase = ImePhase::Compose;
                false
            }
            ImePhase::Compose => {
                let Some(case) = run.cases.front().copied() else {
                    self.ime_run = None;
                    self.log
                        .emit("ime_corpus_completed", json!({ "cases": IME_CORPUS.len() }));
                    return true;
                };
                let replacement = case
                    .replaces_composition
                    .then(|| {
                        self.ime
                            .marked
                            .as_ref()
                            .map(|range| [range.start as u32, range.end as u32])
                    })
                    .flatten();
                self.compose(case.composition, replacement, cx);
                if let Some(run) = self.ime_run.as_mut() {
                    run.phase = ImePhase::Observe {
                        case,
                        at: Instant::now() + Duration::from_millis(350),
                    };
                }
                false
            }
            ImePhase::Observe { case, at } => {
                if Instant::now() < at {
                    return false;
                }
                let mut report = self.composition_report(window);
                report["case"] = json!(case.name);
                report["composition"] = json!(case.composition);
                report["replacement"] = json!(case.replaces_composition);
                match case.resolution {
                    ImeResolution::Commit(text) => {
                        report["resolution"] = json!("commit");
                        let replacement = self
                            .ime
                            .marked
                            .as_ref()
                            .map(|range| [range.start as u32, range.end as u32]);
                        self.input(
                            InputEvent::ImeCommit {
                                text: text.to_string(),
                                replacement,
                            },
                            cx,
                        );
                    }
                    ImeResolution::Cancel => {
                        report["resolution"] = json!("cancel");
                        self.input(InputEvent::ImeCancel, cx);
                        self.observed_cancellations = self.observed_cancellations.saturating_add(1);
                    }
                    ImeResolution::Finish => {
                        report["resolution"] = json!("finish");
                        self.input(InputEvent::ImeFinish, cx);
                    }
                }
                self.ime_composing = false;
                self.ime.finish();
                self.log.emit("ime_case", report);
                if let Some(run) = self.ime_run.as_mut() {
                    run.cases.pop_front();
                    run.phase = ImePhase::Settle {
                        at: Instant::now() + Duration::from_millis(150),
                    };
                }
                false
            }
            ImePhase::Settle { at } => {
                if Instant::now() < at {
                    return false;
                }
                run.phase = ImePhase::Compose;
                false
            }
        }
    }

    fn dpi_percentages(&self) -> VecDeque<u32> {
        self.options
            .dpi_transitions
            .as_deref()
            .map(|list| {
                list.split(',')
                    .filter_map(|item| item.trim().parse::<u32>().ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn step_dpi(&mut self, window: &Window, cx: &mut Context<Self>) -> Result<bool, String> {
        if self.dpi.is_none() {
            let pending = self.dpi_percentages();
            if pending.is_empty() {
                self.log.emit(
                    "dpi_skipped",
                    json!({ "reason": "--dpi-transitions was not requested" }),
                );
                return Ok(true);
            }
            let monitor = window_monitor(window)?;
            if monitor.primary {
                return Err(
                    "the DPI qualification refuses to change the scale of the primary monitor"
                        .to_string(),
                );
            }
            let source = super::display_scale::source_for_device(&monitor.device)?;
            let range = super::display_scale::scale_range(&source)?;
            let baseline_percent = monitor.scale_percent;
            self.log.emit(
                "dpi_started",
                json!({
                    "monitor": monitor.report(),
                    "baseline_percent": baseline_percent,
                    "requested": pending.iter().copied().collect::<Vec<_>>(),
                    "range": { "minimum": range.minimum, "current": range.current, "maximum": range.maximum }
                }),
            );
            self.dpi = Some(DpiRun {
                monitor,
                source,
                range,
                baseline_percent,
                pending,
                phase: DpiPhase::Apply,
                scale: None,
            });
            self.ensure_text_focus(cx);
            return Ok(false);
        }
        let observed = (window.scale_factor() * 100.0).round() as u32;
        let Some(run) = self.dpi.as_mut() else {
            return Ok(true);
        };
        match run.phase {
            DpiPhase::Apply => {
                let Some(wanted) = run.pending.pop_front() else {
                    run.phase = DpiPhase::Restore {
                        deadline: Instant::now() + Duration::from_secs(8),
                    };
                    if let Some(scale) = run.scale.as_mut() {
                        scale.restore()?;
                    }
                    return Ok(false);
                };
                let relative = super::display_scale::relative_for_percent(
                    run.range,
                    run.baseline_percent,
                    wanted,
                )?;
                match run.scale.as_mut() {
                    Some(scale) => scale.set(relative)?,
                    None => {
                        run.scale = Some(super::display_scale::ScaleOverride::apply(
                            &run.source,
                            run.range,
                            relative,
                        )?)
                    }
                }
                run.phase = DpiPhase::Settle {
                    wanted,
                    deadline: Instant::now() + Duration::from_secs(8),
                };
                Ok(false)
            }
            DpiPhase::Settle { wanted, deadline } => {
                if observed != wanted {
                    if Instant::now() > deadline {
                        return Err(format!(
                            "the browser window stayed at {observed}% after requesting {wanted}%"
                        ));
                    }
                    return Ok(false);
                }
                run.phase = DpiPhase::Sample {
                    wanted,
                    at: Instant::now() + Duration::from_millis(500),
                };
                self.compose("にほんご", None, cx);
                Ok(false)
            }
            DpiPhase::Sample { wanted, at } => {
                if Instant::now() < at {
                    return Ok(false);
                }
                let monitor = window_monitor(window)?;
                let mut report = self.composition_report(window);
                report["requested_percent"] = json!(wanted);
                report["monitor"] = monitor.report();
                self.input(InputEvent::ImeCancel, cx);
                self.ime_composing = false;
                self.ime.finish();
                self.observed_cancellations = self.observed_cancellations.saturating_add(1);
                self.log.emit("dpi_scale", report);
                if let Some(run) = self.dpi.as_mut() {
                    run.phase = DpiPhase::Apply;
                }
                Ok(false)
            }
            DpiPhase::Restore { deadline } => {
                if observed != run.baseline_percent {
                    if Instant::now() > deadline {
                        return Err(format!(
                            "the display scale stayed at {observed}% instead of the baseline {}%",
                            run.baseline_percent
                        ));
                    }
                    return Ok(false);
                }
                let monitor = run.monitor.device.clone();
                let baseline = run.baseline_percent;
                self.dpi = None;
                self.log.emit(
                    "dpi_restored",
                    json!({ "monitor": monitor, "scale_percent": baseline }),
                );
                Ok(true)
            }
        }
    }

    fn step_agent(&mut self) -> Result<bool, String> {
        let Some(url) = self.options.agent_url.clone() else {
            return Err("the agent step requires --agent-url".to_string());
        };
        let Some(page) = &self.page else {
            return Err("the browser page closed before the agent step".to_string());
        };
        let run = self.agent_run.get_or_insert_with(|| AgentRun {
            phase: AgentPhase::Navigate,
            operation: None,
            committed_generation: None,
            committed_url: None,
            stale_refused: false,
            cancelled: false,
            takeover_after: None,
            deadline: Instant::now() + Duration::from_secs(30),
        });
        if Instant::now() > run.deadline {
            return Err(format!("the agent step timed out in phase {:?}", run.phase));
        }
        match run.phase {
            AgentPhase::Navigate => {
                let operation = page
                    .send_to_document(|document| Command::AgentNavigate {
                        document,
                        url: url.clone(),
                    })
                    .map_err(|error| format!("agent navigation refused: {error:?}"))?;
                self.log.emit(
                    "agent_navigate",
                    json!({ "operation": operation.as_str(), "url": url }),
                );
                run.operation = Some(operation);
                run.phase = AgentPhase::AwaitCommit;
                Ok(false)
            }
            AgentPhase::AwaitCommit => {
                if run.committed_generation.is_none() {
                    return Ok(false);
                }
                self.log.emit(
                    "agent_committed",
                    json!({
                        "generation": run.committed_generation,
                        "url": run.committed_url,
                    }),
                );
                run.phase = AgentPhase::Stale;
                run.deadline = Instant::now() + Duration::from_secs(30);
                Ok(false)
            }
            AgentPhase::Stale => {
                let Some(document) = page.document().cloned() else {
                    return Err(
                        "the browser page has no document for the stale agent case".to_string()
                    );
                };
                let mut stale = document.clone();
                stale.generation = document.generation.saturating_add(1);
                match page.send(Command::AgentNavigate {
                    document: stale,
                    url: url.clone(),
                }) {
                    Ok(operation) => {
                        self.log.emit(
                            "agent_stale_sent",
                            json!({ "operation": operation.as_str(), "generation": document.generation.saturating_add(1) }),
                        );
                        run.operation = Some(operation);
                        run.phase = AgentPhase::AwaitStale;
                        run.deadline = Instant::now() + Duration::from_secs(30);
                        self.expected_refusals += 1;
                    }
                    Err(error) => {
                        run.stale_refused = true;
                        self.log.emit(
                            "agent_stale_refused",
                            json!({ "error": format!("{error:?}"), "refused_by": "local scope check" }),
                        );
                        run.phase = AgentPhase::Cancel;
                        run.deadline = Instant::now() + Duration::from_secs(30);
                    }
                }
                Ok(false)
            }
            AgentPhase::AwaitStale => {
                if !run.stale_refused {
                    return Ok(false);
                }
                run.phase = AgentPhase::Cancel;
                run.deadline = Instant::now() + Duration::from_secs(30);
                Ok(false)
            }
            AgentPhase::Cancel => {
                let Some(document) = page.document().cloned() else {
                    return Err(
                        "the browser page has no document for the human takeover".to_string()
                    );
                };
                let operation = page
                    .send(Command::AgentNavigate {
                        document: document.clone(),
                        url: url.clone(),
                    })
                    .map_err(|error| format!("second agent navigation refused: {error:?}"))?;
                self.log.emit(
                    "agent_second_navigate",
                    json!({ "operation": operation.as_str(), "generation": document.generation }),
                );
                run.operation = Some(operation);
                run.cancelled = false;
                run.takeover_after = Some(document.generation);
                run.phase = AgentPhase::Takeover;
                run.deadline = Instant::now() + Duration::from_secs(30);
                Ok(false)
            }
            AgentPhase::Takeover => {
                let Some(document) = page.document().cloned() else {
                    return Err(
                        "the browser page has no document for the human takeover".to_string()
                    );
                };
                if run
                    .takeover_after
                    .is_none_or(|generation| document.generation <= generation)
                {
                    return Ok(false);
                }
                let human = self
                    .options
                    .url
                    .clone()
                    .ok_or("the agent step needs the human --url to take the page back")?;
                page.send(Command::Navigate {
                    document: document.clone(),
                    url: human.clone(),
                })
                .map_err(|error| format!("human takeover refused: {error:?}"))?;
                self.log.emit(
                    "agent_takeover",
                    json!({ "generation": document.generation, "human_url": human }),
                );
                run.phase = AgentPhase::AwaitCancel;
                run.deadline = Instant::now() + Duration::from_secs(30);
                Ok(false)
            }
            AgentPhase::AwaitCancel => {
                if !run.cancelled {
                    return Ok(false);
                }
                self.log.emit(
                    "agent_cancelled",
                    json!({ "stale_refused": run.stale_refused }),
                );
                Ok(true)
            }
        }
    }

    fn step_cancel(&mut self, window: &Window, cx: &mut Context<Self>) -> Result<bool, String> {
        let geometry = self.geometry(window);
        match self.cancel_phase {
            None => {
                self.compose("interrompu", None, cx);
                let marked_before = self.ime.marked.clone();
                self.input(InputEvent::ImeFinish, cx);
                self.input(InputEvent::Focus { focused: false }, cx);
                self.ime_composing = false;
                self.ime.finish();
                self.observed_cancellations = self.observed_cancellations.saturating_add(1);
                self.log.emit(
                    "cancellation",
                    json!({
                        "condition": "focus_lost",
                        "marked_before": marked_before.map(|range| [range.start, range.end]),
                        "marked_after": self.ime.marked.as_ref().map(|range| [range.start, range.end]),
                        "composing": self.ime_composing
                    }),
                );
                self.cancel_phase = Some(CancelPhase::Focus);
                Ok(false)
            }
            Some(CancelPhase::Focus) => {
                let x = (geometry.width / 2) as i32;
                let y = (geometry.height / 2) as i32;
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
                self.input(InputEvent::CaptureLost, cx);
                self.observed_cancellations = self.observed_cancellations.saturating_add(1);
                self.log.emit(
                    "cancellation",
                    json!({ "condition": "capture_lost", "x": x, "y": y }),
                );
                self.cancel_phase = Some(CancelPhase::Capture);
                Ok(false)
            }
            Some(CancelPhase::Capture) => {
                let Some(page) = &self.page else {
                    return Err("the browser page closed before the stale input case".to_string());
                };
                let Some(document) = page.document().cloned() else {
                    return Err(
                        "the browser page has no document for the stale input case".to_string()
                    );
                };
                let mut stale = document.clone();
                stale.generation = document.generation.saturating_add(1);
                let input = InputEvent::MouseMove {
                    x: (geometry.width / 2) as i32,
                    y: (geometry.height / 2) as i32,
                    modifiers: 0,
                };
                match page.send(Command::Input {
                    document: stale.clone(),
                    input,
                }) {
                    Ok(operation) => {
                        self.expected_failures.push(operation);
                        self.expected_refusals += 1;
                        self.log.emit(
                            "cancellation_requested",
                            json!({ "condition": "stale_document", "generation": stale.generation, "live_generation": document.generation }),
                        );
                    }
                    Err(error) => {
                        self.observed_cancellations = self.observed_cancellations.saturating_add(1);
                        self.log.emit(
                            "cancellation",
                            json!({ "condition": "stale_document", "error": format!("{error:?}") }),
                        );
                    }
                }
                self.cancel_phase = Some(CancelPhase::Stale {
                    deadline: Instant::now() + Duration::from_secs(5),
                });
                Ok(false)
            }
            Some(CancelPhase::Stale { deadline }) => {
                if self.expected_refusals > 0 || !self.expected_failures.is_empty() {
                    if Instant::now() > deadline {
                        return Err(
                            "the host accepted an input addressed to a stale document".to_string()
                        );
                    }
                    return Ok(false);
                }
                self.cancel_phase = Some(CancelPhase::Viewport);
                Ok(false)
            }
            Some(CancelPhase::Viewport) => {
                let previous = self.last_geometry;
                self.log.emit(
                    "cancellation",
                    json!({
                        "condition": "viewport_coordinates",
                        "previous": previous.map(|geometry| json!({ "width": geometry.width, "height": geometry.height, "scale_percent": geometry.scale_percent })),
                        "current": { "width": geometry.width, "height": geometry.height, "scale_percent": geometry.scale_percent },
                        "center": [(geometry.width / 2) as i32, (geometry.height / 2) as i32],
                        "cancellations": self.observed_cancellations
                    }),
                );
                self.cancel_phase = None;
                Ok(true)
            }
        }
    }

    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.finished {
            return;
        }
        if self.started.elapsed() > RUN_TIMEOUT {
            self.fail("browser prototype exceeded its time budget".to_string(), cx);
            return;
        }
        if let Some(page) = &mut self.page
            && let Err(error) = page.schedule_releases(window, cx)
        {
            self.fail(error, cx);
            return;
        }
        if self.session.is_none() && self.started.elapsed() > STARTUP_TIMEOUT {
            self.fail(
                "browser session did not start within the startup budget".to_string(),
                cx,
            );
            return;
        }
        if self.first_frame.is_none() {
            return;
        }
        if self.step_started.is_none() && self.hold_until.is_none() {
            if let Some(step) = self.steps.front() {
                self.step_started = Some(Instant::now());
                self.log
                    .emit("step_started", json!({ "step": step.name() }));
            } else {
                self.hold_until = Some(Instant::now() + Duration::from_secs(self.options.hold));
                self.log
                    .emit("hold", json!({ "seconds": self.options.hold }));
            }
        }
        if let Some(until) = self.hold_until {
            if Instant::now() >= until {
                self.finish(cx);
            } else {
                let timer = cx.background_executor().timer(TICK);
                cx.spawn(async move |view, cx| {
                    timer.await;
                    let _ = view.update(cx, |_, cx| cx.notify());
                })
                .detach();
            }
            return;
        }
        let Some(step) = self.steps.front().copied() else {
            return;
        };
        if self
            .step_started
            .is_some_and(|started| started.elapsed() > STARTUP_TIMEOUT)
        {
            self.fail(format!("browser step {} timed out", step.name()), cx);
            return;
        }
        let completed = match step {
            Step::Input => {
                self.step_input(window, cx);
                true
            }
            Step::Wheel => {
                self.step_wheel(window, cx);
                true
            }
            Step::Drag => {
                self.step_drag(window, cx);
                true
            }
            Step::Resize => {
                self.step_resize(window);
                true
            }
            Step::Scale => match self.step_scale(window) {
                Ok(()) => true,
                Err(error) => {
                    self.fail(format!("Windows monitor transition failed: {error}"), cx);
                    return;
                }
            },
            Step::Dpi => match self.step_dpi(window, cx) {
                Ok(completed) => completed,
                Err(error) => {
                    self.fail(format!("Windows DPI transition failed: {error}"), cx);
                    return;
                }
            },
            Step::Ime => self.step_ime(window, cx),
            Step::Cancel => match self.step_cancel(window, cx) {
                Ok(completed) => completed,
                Err(error) => {
                    self.fail(format!("Windows input cancellation failed: {error}"), cx);
                    return;
                }
            },
            Step::Agent => match self.step_agent() {
                Ok(completed) => completed,
                Err(error) => {
                    self.fail(format!("Windows agent control failed: {error}"), cx);
                    return;
                }
            },
        };
        if !completed {
            let timer = cx.background_executor().timer(TICK);
            cx.spawn(async move |view, cx| {
                timer.await;
                let _ = view.update(cx, |_, cx| cx.notify());
            })
            .detach();
            return;
        }
        self.log
            .emit("step_completed", json!({ "step": step.name() }));
        self.steps.pop_front();
        self.step_started = None;
    }

    fn position(point: gpui::Point<Pixels>) -> (i32, i32) {
        (
            f32::from(point.x).round() as i32,
            f32::from(point.y).round() as i32,
        )
    }

    fn mouse_button(
        &mut self,
        button: gpui::MouseButton,
        point: gpui::Point<Pixels>,
        down: bool,
        clicks: usize,
        held: &Modifiers,
        cx: &mut Context<Self>,
    ) {
        let Some(button) = mouse_button(button) else {
            return;
        };
        let (x, y) = Self::position(point);
        self.input(
            InputEvent::MouseButton {
                x,
                y,
                button,
                down,
                clicks: clicks.clamp(1, 3) as u8,
                modifiers: modifiers(held),
            },
            cx,
        );
    }
}

impl Render for PrototypeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.initialize(window, cx);
        self.present(window, cx);
        self.tick(window, cx);
        let surface = self.page.as_ref().and_then(LivePage::surface);
        let view = cx.entity();
        let focus = self.focus.clone();
        let root = div()
            .id("browser-prototype")
            .size_full()
            .bg(gpui::rgb(0x1a1a1a))
            .track_focus(&self.focus)
            .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, _, cx| {
                let (x, y) = Self::position(event.position);
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
                cx.listener(|view, event: &MouseDownEvent, window, cx| {
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
                cx.listener(|view, event: &MouseDownEvent, _, cx| {
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
                cx.listener(|view, event: &MouseDownEvent, _, cx| {
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
                cx.listener(|view, event: &MouseUpEvent, _, cx| {
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
                cx.listener(|view, event: &MouseUpEvent, _, cx| {
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
                cx.listener(|view, event: &MouseUpEvent, _, cx| {
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
            .on_scroll_wheel(cx.listener(|view, event: &ScrollWheelEvent, _, cx| {
                let (x, y) = Self::position(event.position);
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
                if view.ime_composing {
                    return;
                }
                for input in consume_key_down(event, cx) {
                    view.input(input, cx);
                }
            }))
            .on_key_up(cx.listener(|view, event: &KeyUpEvent, _, cx| {
                for input in key_events(&event.keystroke, false) {
                    view.input(input, cx);
                }
                cx.stop_propagation();
            }));
        if let Some(surface) = surface {
            root.child(
                gpui::canvas(
                    |_, _, _| (),
                    move |bounds, (), window, cx| {
                        window.paint_external_surface(bounds, surface);
                        if focus.is_focused(window) {
                            window.handle_input(
                                &focus,
                                WindowsInputHandler {
                                    view: view.clone(),
                                    bounds,
                                },
                                cx,
                            );
                        }
                    },
                )
                .size_full(),
            )
        } else {
            root
        }
    }
}

struct WindowsInputHandler {
    view: Entity<PrototypeView>,
    bounds: Bounds<Pixels>,
}

impl gpui::InputHandler for WindowsInputHandler {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<gpui::UTF16Selection> {
        let range = self.view.read(cx).ime.selection.clone()?;
        Some(gpui::UTF16Selection {
            range: range.start.min(range.end)..range.start.max(range.end),
            reversed: range.start > range.end,
        })
    }

    fn marked_text_range(&mut self, _window: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        self.view.read(cx).ime.marked.clone()
    }

    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<String> {
        let text = self.view.read(cx).ime.text_for_range(range_utf16.clone())?;
        *adjusted_range = Some(range_utf16);
        Some(text)
    }

    fn replace_text_in_range(
        &mut self,
        replacement_range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut App,
    ) {
        let replacement = super::ime::replacement_range(replacement_range)
            .ok()
            .flatten();
        self.view.update(cx, |view, cx| {
            view.input(
                InputEvent::ImeCommit {
                    text: text.to_string(),
                    replacement,
                },
                cx,
            );
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut App,
    ) {
        let replacement = super::ime::replacement_range(range_utf16).ok().flatten();
        let selection = new_selected_range
            .and_then(|range| u32::try_from(range.end).ok().map(|end| (range, end)))
            .map(|(range, _)| range);
        let cursor = selection.as_ref().map_or(0, |range| range.end) as u32;
        let selection_start = selection.as_ref().map(|range| range.start as u32);
        self.view.update(cx, |view, cx| {
            view.ime_composing = !new_text.is_empty();
            view.ime.compose(new_text, selection.clone());
            view.input(
                InputEvent::ImeComposition {
                    text: new_text.to_string(),
                    cursor,
                    selection_start,
                    replacement,
                },
                cx,
            );
        });
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut App) {
        self.view.update(cx, |view, cx| {
            view.ime_composing = false;
            view.input(InputEvent::ImeFinish, cx);
            view.ime.finish();
        });
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        let view = self.view.read(cx);
        let rect = view.ime.caret_bounds(range_utf16.start)?;
        let geometry = view.last_geometry?;
        Some(super::ime::candidate_bounds(
            rect,
            self.bounds.origin,
            self.bounds.size,
            geometry,
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<usize> {
        None
    }
}

fn open_window(
    options: Arc<Options>,
    log: Logger,
    exit: Arc<AtomicI32>,
    cx: &mut App,
) -> Result<(), String> {
    let dimensions = size(px(options.width as f32), px(options.height as f32));
    let bounds = Bounds::centered(None, dimensions, cx);
    let window_options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        app_id: Some("paneflow-browser-prototype".into()),
        focus: true,
        inactive_frame_interval: None,
        ..Default::default()
    };
    cx.open_window(window_options, move |window, cx| {
        let view = cx.new(|cx| PrototypeView::new(options, log, exit, cx));
        let focus = view.read(cx).focus.clone();
        window.focus(&focus, cx);
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
        view
    })
    .map(|_| ())
    .map_err(|error| error.to_string())
}

pub fn run(args: &[String]) -> i32 {
    let mut argv = vec!["paneflow browser-prototype".to_string()];
    argv.extend(args.iter().skip(2).cloned());
    let options = match Options::try_parse_from(argv) {
        Ok(options) => Arc::new(options),
        Err(error) => {
            let _ = error.print();
            return 2;
        }
    };
    if options.source != "cef" {
        eprintln!("--source must be cef on Windows");
        return 2;
    }
    if options.url.is_none() {
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
            "scenario": options.scenario,
            "dpi_transitions": options.dpi_transitions,
            "clock": "process_relative_monotonic",
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        }),
    );
    log.emit("machine", machine_report());
    let exit = Arc::new(AtomicI32::new(1));
    let exit_for_app = exit.clone();
    gpui_platform::application().run(move |cx: &mut App| {
        BrowserRuntime::install(cx);
        if let Err(error) = open_window(options, log, exit_for_app, cx) {
            eprintln!("browser prototype window failed: {error}");
            cx.quit();
        } else {
            cx.activate(true);
        }
    });
    exit.load(Ordering::SeqCst)
}
