use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, mpsc};

use gpui::{App, Window};
use serde_json::{Value, json};
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

static LOG: OnceLock<Option<mpsc::SyncSender<Value>>> = OnceLock::new();
static DROPPED: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static INPUTS: RefCell<BTreeMap<u64, Input>> = const { RefCell::new(BTreeMap::new()) };
    static WINDOWS: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

#[derive(Default)]
struct Input {
    line: String,
    started: u64,
    key_ns: Option<u64>,
    pending: VecDeque<(u64, u64)>,
}

pub(crate) fn now_ns() -> u64 {
    let mut counter = 0;
    let mut frequency = 0;
    if unsafe { QueryPerformanceCounter(&mut counter) } == 0
        || unsafe { QueryPerformanceFrequency(&mut frequency) } == 0
        || counter < 0
        || frequency <= 0
    {
        return 0;
    }
    ((counter as u128 * 1_000_000_000) / frequency as u128) as u64
}

pub(crate) fn enabled() -> bool {
    LOG.get_or_init(|| {
        let path = std::path::PathBuf::from(std::env::var_os("PANEFLOW_M1_LOG")?);
        if !path.is_absolute() {
            return None;
        }
        let (sender, receiver) = mpsc::sync_channel::<Value>(8192);
        std::thread::Builder::new()
            .name("windows-m1-log".into())
            .spawn(move || {
                let Ok(file) = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
                else {
                    eprintln!("M1 evidence cannot be created");
                    return;
                };
                let mut file = std::io::BufWriter::new(file);
                for value in receiver {
                    if serde_json::to_writer(&mut file, &value).is_err()
                        || writeln!(file).is_err()
                        || file.flush().is_err()
                    {
                        break;
                    }
                }
            })
            .ok()?;
        Some(sender)
    })
    .is_some()
}

pub(crate) fn record(event: &str, fields: Value) {
    if !enabled() {
        return;
    }
    if let Some(Some(sender)) = LOG.get()
        && sender.try_send(json!({"event":event,"at_ns":now_ns(),"pid":std::process::id(),"clock":"QPC","fields":fields,"dropped":DROPPED.load(Ordering::Relaxed)})).is_err()
    {
        DROPPED.fetch_add(1, Ordering::Relaxed);
        eprintln!("M1 evidence queue overflow: capture invalid");
    }
}

struct Observer;

impl gpui::RendererTimingObserver for Observer {
    fn now_ns(&self) -> Option<u64> {
        Some(now_ns())
    }

    fn record(&self, timing: gpui::RendererTiming) {
        record(
            "renderer_stage",
            json!({"window_id":timing.identity.window_id,
            "attempt":timing.attempt,"stage":timing.stage,"start_ns":timing.start_ns,
            "end_ns":timing.end_ns,"outcome":timing.outcome,
            "width":timing.width,"height":timing.height}),
        );
    }
}

pub(crate) fn observe(window: &Window) {
    if !enabled() {
        return;
    }
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let id = handle.hwnd.get() as u64;
    WINDOWS.with_borrow_mut(|windows| {
        if windows.contains(&id) {
            return;
        }
        windows.push(id);
        window.set_renderer_timing_observer(Some(Arc::new(Observer)));
        let viewport = window.viewport_size();
        record(
            "viewport",
            json!({"window_id":id,"scale":window.scale_factor(),
            "width":f32::from(viewport.width),"height":f32::from(viewport.height),
            "presentation_boundary":"external PresentMon ETW trace, joined to dxgi_present_call"}),
        );
    });
}

pub(crate) fn input_text(surface_id: u64, text: &str) {
    if !enabled() {
        return;
    }
    INPUTS.with_borrow_mut(|inputs| {
        let input = inputs.entry(surface_id).or_default();
        if input.line.is_empty() {
            input.started = input.key_ns.take().unwrap_or_else(now_ns);
        }
        if input.line.len() + text.len() <= 32 {
            input.line.push_str(text);
        } else {
            input.line.clear();
            paint_failed("input marker limit");
        }
    });
}

pub(crate) fn input_key(surface_id: u64, key: &str) {
    if !enabled() {
        return;
    }
    INPUTS.with_borrow_mut(|inputs| {
        let input = inputs.entry(surface_id).or_default();
        input.key_ns = Some(now_ns());
        if key != "enter" {
            return;
        }
        let line = std::mem::take(&mut input.line);
        if let Some(tick) = line
            .strip_prefix('i')
            .and_then(|value| value.parse::<u64>().ok())
        {
            if input.pending.len() >= 32 {
                paint_failed("input queue overflow");
                return;
            }
            input.pending.push_back((tick, input.started));
            record(
                "input",
                json!({"surface_id":surface_id,"tick":tick,"input_ns":input.started}),
            );
        }
    });
}

pub(crate) fn cpu_started() -> u64 {
    now_ns()
}

pub(crate) fn cpu_finished(surface_id: u64, phase: &str, start: u64) {
    record(
        "cpu_span",
        json!({"surface_id":surface_id,"phase":phase,"start_ns":start,
        "end_ns":now_ns(),"metric":"wall_span_not_thread_cpu"}),
    );
}

pub(crate) fn paint_failed(reason: impl std::fmt::Display) {
    record("fatal", json!({"reason":reason.to_string()}));
}

pub(crate) fn painted<'a>(
    surface_id: u64,
    runs: impl Iterator<Item = (i32, usize, &'a str)>,
    columns: usize,
    rows: usize,
    window: &Window,
    _cx: &App,
) {
    if !enabled() {
        return;
    }
    observe(window);
    if !INPUTS.with_borrow(|inputs| {
        inputs
            .get(&surface_id)
            .is_some_and(|input| !input.pending.is_empty())
    }) {
        return;
    }
    let mut visible = vec![vec![' '; columns.min(512)]; rows.min(256)];
    for (line, column, text) in runs {
        let Ok(line) = usize::try_from(line) else {
            continue;
        };
        if let Some(row) = visible.get_mut(line) {
            for (offset, character) in text.chars().enumerate() {
                if let Some(cell) = row.get_mut(column + offset) {
                    *cell = character;
                }
            }
        }
    }
    INPUTS.with_borrow_mut(|inputs| {
        let Some(input) = inputs.get_mut(&surface_id) else {
            return;
        };
        for row in visible {
            let row: String = row.into_iter().collect();
            let Some((_, marker)) = row.split_once("pf-input:") else {
                continue;
            };
            let Some((terminal, tick)) = marker.split_once(':') else {
                continue;
            };
            let tick = tick
                .split(|character: char| !character.is_ascii_digit())
                .next()
                .and_then(|tick| tick.parse::<u64>().ok());
            let Some(tick) = tick else {
                continue;
            };
            if let Some(index) = input.pending.iter().position(|pending| pending.0 == tick)
                && let Some((_, start)) = input.pending.remove(index)
            {
                record(
                    "echo_scene",
                    json!({"surface_id":surface_id,"terminal":terminal,
                    "tick":tick,"input_ns":start,"columns":columns,"rows":rows}),
                );
            }
        }
    });
}
