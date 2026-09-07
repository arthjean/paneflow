use std::cell::RefCell;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use cef::*;
use paneflow_browser_protocol::{
    read_message, write_message, BrowserError, Command, Controller, Envelope, Event, FrameChannel,
    HistoryDirection, Owner, CONTRACT_VERSION, FRAME_CHANNEL_ENV,
};
use serde_json::json;

mod clipboard;
mod clipboard_renderer;
mod editing;
mod gpu_contract;
mod handlers;
mod presentation;
mod views;
mod vulkan;

const GPU_ENV: &str = "PANEFLOW_BROWSER_GPU";

struct Host {
    controller: Controller,
    owner: Owner,
    receiver: Receiver<Option<Envelope>>,
    browser: Option<Browser>,
    window: Option<Window>,
    origin: String,
    closing: bool,
    tracing: bool,
    trace_path: PathBuf,
}

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

pub(crate) fn now_ns() -> u64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) };
    time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64
}

fn parse_gpu(value: &str) -> Result<(u32, u32), String> {
    let (vendor, device) = value
        .split_once(':')
        .ok_or_else(|| format!("{GPU_ENV} must be vendor:device in hexadecimal"))?;
    let parse = |part: &str| {
        u32::from_str_radix(part.trim_start_matches("0x"), 16)
            .map_err(|_| format!("{GPU_ENV} must be vendor:device in hexadecimal"))
    };
    Ok((parse(vendor)?, parse(device)?))
}

fn emit(mut value: serde_json::Value) {
    value["trace_us"] = json!(now_from_system_trace_time());
    if write_message(&mut io::stdout().lock(), &value).is_err() {
        quit_message_loop();
    }
}

fn close_browser() {
    editing::clear();
    let browser = HOST.with(|state| {
        let mut state = state.borrow_mut();
        state.as_mut().and_then(|host| {
            host.closing = true;
            host.browser.clone()
        })
    });
    if let Some(browser) = browser {
        if let Some(host) = browser.host() {
            emit(json!({ "native": "close_requested" }));
            host.close_browser(1);
            emit(json!({ "native": "close_dispatched" }));
        }
    } else {
        quit_message_loop();
    }
}

fn close() {
    if HOST.with(|state| state.borrow().as_ref().is_some_and(|host| host.closing)) {
        return;
    }
    let trace_path = HOST.with(|state| {
        let mut state = state.borrow_mut();
        state.as_mut().and_then(|host| {
            if !host.tracing {
                return None;
            }
            host.tracing = false;
            host.closing = true;
            Some(host.trace_path.clone())
        })
    });
    if let Some(path) = trace_path {
        let path = CefString::from(path.to_string_lossy().as_ref());
        emit(json!({ "native": "trace_stop_requested" }));
        if end_tracing(Some(&path), Some(&mut TraceFinished::new())) == 1 {
            return;
        }
        emit(json!({ "native": "trace_failed", "reason": "end_tracing rejected" }));
    }
    close_browser();
}

wrap_completion_callback! {
    struct TraceStarted;

    impl CompletionCallback {
        fn on_complete(&self) {
            emit(json!({ "native": "trace_started", "trace_us": now_from_system_trace_time() }));
            post_task(ThreadId::UI, Some(&mut DrainControl::new()));
        }
    }
}

wrap_end_tracing_callback! {
    struct TraceFinished;

    impl EndTracingCallback {
        fn on_end_tracing_complete(&self, tracing_file: Option<&CefString>) {
            emit(json!({ "native": "trace_completed", "path": tracing_file.map(ToString::to_string), "trace_us": now_from_system_trace_time() }));
            close_browser();
        }
    }
}

fn process(message: Envelope) {
    let command = message.command.clone();
    let reply = HOST.with(|state| {
        let mut state = state.borrow_mut();
        state.as_mut().map(|host| {
            let permitted = match &command {
                Command::Create { url, .. } => url.starts_with(&format!("{}/", host.origin)),
                Command::Navigate { url, .. } => {
                    presentation::active() || url.starts_with(&format!("{}/", host.origin))
                }
                Command::Capabilities | Command::State { .. } | Command::Close { .. } => true,
                Command::Start { .. } => host.browser.is_none(),
                Command::Present { .. } | Command::Input { .. } => presentation::active(),
                Command::History { .. }
                | Command::Reload { .. }
                | Command::Stop { .. }
                | Command::Zoom { .. }
                | Command::Mute { .. } => host.browser.is_some(),
                _ => false,
            };
            if permitted && !host.closing {
                host.controller.dispatch(&host.owner, message)
            } else {
                paneflow_browser_protocol::Reply {
                    version: CONTRACT_VERSION,
                    operation: message.operation,
                    result: Err(BrowserError::Unavailable),
                }
            }
        })
    });
    let Some(mut reply) = reply else {
        return;
    };
    if let Ok(Event::State { session, .. }) = &reply.result {
        presentation::set_document(&session.document);
        if matches!(command, Command::Start { .. }) {
            emit(json!({ "native": "browser_create_requested", "document": session.document }));
            let created = if presentation::active() {
                presentation::create(&session.url)
            } else {
                views::create(&session.url)
            };
            if !created {
                reply.result = Err(BrowserError::Unavailable);
            }
        }
    }
    let succeeded = reply.result.is_ok();
    if let Ok(value) = serde_json::to_value(reply) {
        emit(json!({ "protocol": value }));
    }
    if succeeded {
        match command {
            Command::Close { .. } => close(),
            Command::Navigate { url, .. } => {
                if let Some(frame) = current_browser().and_then(|browser| browser.main_frame()) {
                    frame.load_url(Some(&url.as_str().into()));
                }
                presentation::unmount();
            }
            Command::Present { presentation, .. } => presentation::present(&presentation),
            Command::Input { input, .. } => presentation::input(input),
            Command::History { direction, .. } => {
                if let Some(browser) = current_browser() {
                    match direction {
                        HistoryDirection::Back => browser.go_back(),
                        HistoryDirection::Forward => browser.go_forward(),
                    }
                }
            }
            Command::Reload { ignore_cache, .. } => {
                if let Some(browser) = current_browser() {
                    if ignore_cache {
                        browser.reload_ignore_cache();
                    } else {
                        browser.reload();
                    }
                }
            }
            Command::Stop { .. } => {
                if let Some(browser) = current_browser() {
                    browser.stop_load();
                }
            }
            Command::Zoom { percent, .. } => {
                if let Some(host) = current_browser().and_then(|browser| browser.host()) {
                    host.set_zoom_level((f64::from(percent) / 100.0).log(1.2));
                }
            }
            Command::Mute { muted, .. } => {
                if let Some(host) = current_browser().and_then(|browser| browser.host()) {
                    host.set_audio_muted(i32::from(muted));
                }
            }
            _ => (),
        }
    }
}

fn current_browser() -> Option<Browser> {
    HOST.with(|state| {
        state
            .borrow()
            .as_ref()
            .and_then(|host| host.browser.clone())
    })
}

wrap_task! {
    struct DrainControl;

    impl Task {
        fn execute(&self) {
            for _ in 0..64 {
                let next = HOST.with(|state| {
                    let state = state.borrow();
                    state.as_ref().filter(|host| !host.closing).map(|host| host.receiver.try_recv())
                });
                match next {
                    Some(Ok(Some(message))) => process(message),
                    Some(Ok(None) | Err(TryRecvError::Disconnected)) => { close(); return; }
                    _ => return,
                }
            }
            post_task(ThreadId::UI, Some(&mut DrainControl::new()));
        }
    }
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|arg| crate::qualification::sandbox_disabled(&arg)) {
        return Err("sandbox disabling switches are forbidden".into());
    }
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let args = cef::args::Args::new();
    let mut app = handlers::WitnessApp::new();
    let result = execute_process(
        Some(args.as_main_args()),
        Some(&mut app),
        std::ptr::null_mut(),
    );
    if result >= 0 {
        std::process::exit(result);
    }
    let root =
        PathBuf::from(std::env::var_os("PANEFLOW_CEF_ROOT").ok_or("runtime root is required")?)
            .canonicalize()?;
    let profile = PathBuf::from(
        std::env::var_os("PANEFLOW_CEF_PROFILE").ok_or("isolated witness profile is required")?,
    );
    let metadata = std::fs::symlink_metadata(&profile)?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err("witness profile must be a private directory (0700)".into());
    }
    let frame_channel = match FrameChannel::from_environment() {
        Ok(channel) => Some(channel),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("{FRAME_CHANNEL_ENV}: {error}").into()),
    };
    let expected_gpu = match std::env::var(GPU_ENV) {
        Ok(value) => Some(parse_gpu(&value)?),
        Err(_) => None,
    };
    let origin = std::env::var("PANEFLOW_CEF_ORIGIN")?;
    match origin.strip_prefix("http://127.0.0.1:") {
        Some(port) => {
            if port.parse::<u16>()? == 0 {
                return Err("origin requires a bound port".into());
            }
        }
        None => {
            let secure = frame_channel.is_some()
                && origin
                    .strip_prefix("https://")
                    .is_some_and(|host| !host.is_empty() && !host.contains('/'));
            if !secure {
                return Err("origin must be a loopback http origin or an https origin".into());
            }
        }
    }
    let owner = match std::env::var("PANEFLOW_BROWSER_OWNER") {
        Ok(value) => {
            let (workspace, session) = value
                .split_once('/')
                .ok_or("PANEFLOW_BROWSER_OWNER must be workspace/session")?;
            Owner {
                workspace: workspace.to_string().try_into()?,
                session: session.to_string().try_into()?,
            }
        }
        Err(_) => Owner {
            workspace: "witness".to_string().try_into()?,
            session: "qualification".to_string().try_into()?,
        },
    };
    let handshake = read_message(&mut io::stdin().lock())
        .map_err(|error| format!("handshake: {error:?}"))?
        .ok_or("missing handshake")?;
    if !matches!(handshake.command, Command::Capabilities) {
        return Err("first control message must negotiate capabilities".into());
    }
    let (sender, receiver) = mpsc::sync_channel(256);
    let mut controller = Controller::new(format!("{}-linux", std::env::consts::ARCH), true);
    let hello = controller.dispatch(&owner, handshake);
    HOST.with(|state| {
        *state.borrow_mut() = Some(Host {
            controller,
            owner,
            receiver,
            browser: None,
            window: None,
            origin,
            closing: false,
            tracing: false,
            trace_path: std::env::var_os("PANEFLOW_BROWSER_TRACE_DIR")
                .map(PathBuf::from)
                .map(|dir| dir.join(format!("cef-{}.json", std::process::id())))
                .unwrap_or_else(|| profile.join("chromium-trace.json")),
        })
    });
    let settings = Settings {
        no_sandbox: 0,
        command_line_args_disabled: 1,
        windowless_rendering_enabled: i32::from(frame_channel.is_some()),
        log_severity: if std::env::var_os("PANEFLOW_BROWSER_LOG_VERBOSE").is_some() {
            LogSeverity::VERBOSE
        } else {
            LogSeverity::default()
        },
        root_cache_path: profile.to_string_lossy().as_ref().into(),
        cache_path: profile.join("profile").to_string_lossy().as_ref().into(),
        resources_dir_path: root.join("Resources").to_string_lossy().as_ref().into(),
        locales_dir_path: root
            .join("Resources/locales")
            .to_string_lossy()
            .as_ref()
            .into(),
        log_file: profile.join("cef.log").to_string_lossy().as_ref().into(),
        ..Default::default()
    };
    if initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut app),
        std::ptr::null_mut(),
    ) != 1
    {
        return Err("CEF initialize failed; inspect witness cef.log".into());
    }
    let presenting = frame_channel.is_some();
    if let Some(channel) = frame_channel {
        presentation::install(channel, expected_gpu)?;
    }
    emit(
        json!({ "native": "initialized", "pid": std::process::id(), "sandbox_requested": true, "contract_version": CONTRACT_VERSION, "presentation": presenting, "protocol": hello }),
    );
    if !std::env::var("PANEFLOW_BROWSER_TRACE").is_ok_and(|value| value == "0") {
        let categories = CefString::from(
            if std::env::var_os("PANEFLOW_BROWSER_TRACE_DIR").is_some() {
                "cef,cc,viz,gpu.capture,blink,devtools.timeline,renderer.scheduler"
            } else {
                "viz,gpu.capture"
            },
        );
        if begin_tracing(Some(&categories), Some(&mut TraceStarted::new())) != 1 {
            return Err("CEF tracing initialization failed".into());
        }
        HOST.with(|state| {
            if let Some(host) = state.borrow_mut().as_mut() {
                host.tracing = true;
            }
        });
    }
    std::thread::spawn(move || {
        let mut input = io::stdin().lock();
        loop {
            match read_message(&mut input) {
                Ok(Some(message)) => {
                    if sender.send(Some(message)).is_err() {
                        return;
                    }
                    post_task(ThreadId::UI, Some(&mut DrainControl::new()));
                }
                Ok(None) => {
                    if sender.send(None).is_ok() {
                        post_task(ThreadId::UI, Some(&mut DrainControl::new()));
                    }
                    return;
                }
                Err(error) => {
                    eprintln!("control rejected: {error:?}");
                    if sender.send(None).is_ok() {
                        post_task(ThreadId::UI, Some(&mut DrainControl::new()));
                    }
                    return;
                }
            }
        }
    });
    run_message_loop();
    presentation::uninstall();
    HOST.with(|state| *state.borrow_mut() = None);
    shutdown();
    emit(json!({ "native": "shutdown" }));
    Ok(())
}
