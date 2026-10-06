use std::alloc::{GlobalAlloc, Layout, System};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, Instant};

struct CountingAllocator;

static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static ALLOCATION_CALLS: AtomicU64 = AtomicU64::new(0);
static LIVE_BYTES: AtomicI64 = AtomicI64::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        ALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
        LIVE_BYTES.fetch_add(layout.size() as i64, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        ALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
        LIVE_BYTES.fetch_add(layout.size() as i64, Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE_BYTES.fetch_sub(layout.size() as i64, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATED_BYTES.fetch_add(
            new_size.saturating_sub(layout.size()) as u64,
            Ordering::Relaxed,
        );
        ALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
        LIVE_BYTES.fetch_add(new_size as i64 - layout.size() as i64, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

pub(crate) fn allocation_counters() -> (u64, u64) {
    (
        ALLOCATED_BYTES.load(Ordering::Relaxed),
        ALLOCATION_CALLS.load(Ordering::Relaxed),
    )
}

pub(crate) fn live_bytes() -> i64 {
    LIVE_BYTES.load(Ordering::Relaxed)
}

const TREE_SITTER_HEADER: usize = 16;

static TREE_SITTER_LIVE_BYTES: AtomicI64 = AtomicI64::new(0);

fn tree_sitter_layout(size: usize) -> Option<Layout> {
    Layout::from_size_align(size.checked_add(TREE_SITTER_HEADER)?, TREE_SITTER_HEADER).ok()
}

unsafe fn tree_sitter_hand_out(block: *mut u8, size: usize) -> *mut c_void {
    if block.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        block.cast::<usize>().write(size);
        TREE_SITTER_LIVE_BYTES.fetch_add(size as i64, Ordering::Relaxed);
        block.add(TREE_SITTER_HEADER).cast()
    }
}

unsafe extern "C" fn tree_sitter_malloc(size: usize) -> *mut c_void {
    let Some(layout) = tree_sitter_layout(size) else {
        return std::ptr::null_mut();
    };
    unsafe { tree_sitter_hand_out(System.alloc(layout), size) }
}

unsafe extern "C" fn tree_sitter_calloc(count: usize, size: usize) -> *mut c_void {
    let Some(total) = count.checked_mul(size) else {
        return std::ptr::null_mut();
    };
    let Some(layout) = tree_sitter_layout(total) else {
        return std::ptr::null_mut();
    };
    unsafe { tree_sitter_hand_out(System.alloc_zeroed(layout), total) }
}

unsafe extern "C" fn tree_sitter_realloc(ptr: *mut c_void, size: usize) -> *mut c_void {
    if ptr.is_null() {
        return unsafe { tree_sitter_malloc(size) };
    }
    unsafe {
        let block = ptr.cast::<u8>().sub(TREE_SITTER_HEADER);
        let held = block.cast::<usize>().read();
        let (Some(layout), Some(next_layout)) =
            (tree_sitter_layout(held), tree_sitter_layout(size))
        else {
            return std::ptr::null_mut();
        };
        let next = System.realloc(block, layout, next_layout.size());
        if next.is_null() {
            return std::ptr::null_mut();
        }
        TREE_SITTER_LIVE_BYTES.fetch_sub(held as i64, Ordering::Relaxed);
        tree_sitter_hand_out(next, size)
    }
}

unsafe extern "C" fn tree_sitter_free(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        let block = ptr.cast::<u8>().sub(TREE_SITTER_HEADER);
        let held = block.cast::<usize>().read();
        let Some(layout) = tree_sitter_layout(held) else {
            return;
        };
        TREE_SITTER_LIVE_BYTES.fetch_sub(held as i64, Ordering::Relaxed);
        System.dealloc(block, layout);
    }
}

pub(crate) unsafe fn count_tree_sitter_allocations_in_a_process_that_has_not_parsed_yet() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| unsafe {
        tree_sitter::set_allocator(
            Some(tree_sitter_malloc),
            Some(tree_sitter_calloc),
            Some(tree_sitter_realloc),
            Some(tree_sitter_free),
        );
    });
}

pub(crate) fn tree_sitter_live_bytes() -> i64 {
    TREE_SITTER_LIVE_BYTES.load(Ordering::Relaxed)
}

#[derive(Clone, Copy)]
pub(crate) enum Direction {
    LowerIsBetter,
    HigherIsBetter,
}

pub(crate) struct Metric {
    pub(crate) name: &'static str,
    pub(crate) unit: &'static str,
    pub(crate) direction: Direction,
    pub(crate) value: f64,
    pub(crate) p95: Option<f64>,
    pub(crate) p99: Option<f64>,
    pub(crate) mean: Option<f64>,
    pub(crate) alloc_bytes_per_iter: Option<f64>,
    pub(crate) allocs_per_iter: Option<f64>,
    pub(crate) iters: usize,
    pub(crate) note: &'static str,
    pub(crate) available: bool,
    pub(crate) samples_ns: Vec<f64>,
}

impl Metric {
    pub(crate) fn count(
        name: &'static str,
        unit: &'static str,
        value: f64,
        note: &'static str,
    ) -> Self {
        Self {
            name,
            unit,
            direction: Direction::LowerIsBetter,
            value,
            p95: None,
            p99: None,
            mean: None,
            alloc_bytes_per_iter: None,
            allocs_per_iter: None,
            iters: 1,
            note,
            available: true,
            samples_ns: Vec::new(),
        }
    }

    pub(crate) fn unavailable(name: &'static str, unit: &'static str, note: &'static str) -> Self {
        Self {
            name,
            unit,
            direction: Direction::LowerIsBetter,
            value: 0.0,
            p95: None,
            p99: None,
            mean: None,
            alloc_bytes_per_iter: None,
            allocs_per_iter: None,
            iters: 0,
            note,
            available: false,
            samples_ns: Vec::new(),
        }
    }

    pub(crate) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "metric": self.name,
            "unit": self.unit,
            "direction": match self.direction {
                Direction::LowerIsBetter => "lower_is_better",
                Direction::HigherIsBetter => "higher_is_better",
            },
            "available": self.available,
            "value": self.available.then_some(self.value),
            "p95": self.p95,
            "p99": self.p99,
            "mean": self.mean,
            "alloc_bytes_per_iter": self.alloc_bytes_per_iter,
            "allocs_per_iter": self.allocs_per_iter,
            "iters": self.iters,
            "note": self.note,
        })
    }
}

fn from_samples(
    name: &'static str,
    note: &'static str,
    samples: &mut [Duration],
    total: Duration,
    allocated: (u64, u64),
    iters: usize,
) -> Metric {
    let samples_ns = samples
        .iter()
        .map(|sample| sample.as_nanos() as f64)
        .collect();
    samples.sort_unstable();
    let iters_f = iters.max(1) as f64;
    Metric {
        name,
        unit: "ns",
        direction: Direction::LowerIsBetter,
        value: percentile_duration(samples, 50).as_nanos() as f64,
        p95: Some(percentile_duration(samples, 95).as_nanos() as f64),
        p99: Some(percentile_duration(samples, 99).as_nanos() as f64),
        mean: Some(total.as_nanos() as f64 / iters_f),
        alloc_bytes_per_iter: Some(allocated.0 as f64 / iters_f),
        allocs_per_iter: Some(allocated.1 as f64 / iters_f),
        iters,
        note,
        available: true,
        samples_ns,
    }
}

pub(crate) fn measure(
    name: &'static str,
    note: &'static str,
    warmup: usize,
    iters: usize,
    mut op: impl FnMut(),
) -> Metric {
    for _ in 0..warmup {
        op();
    }
    let mut samples = Vec::with_capacity(iters);
    let (bytes_before, calls_before) = allocation_counters();
    let started = Instant::now();
    for _ in 0..iters {
        let iteration = Instant::now();
        op();
        samples.push(iteration.elapsed());
    }
    let total = started.elapsed();
    let (bytes_after, calls_after) = allocation_counters();
    from_samples(
        name,
        note,
        &mut samples,
        total,
        (bytes_after - bytes_before, calls_after - calls_before),
        iters,
    )
}

#[derive(Default)]
pub(crate) struct SegmentTimer {
    elapsed: Duration,
    bytes: u64,
    calls: u64,
}

impl SegmentTimer {
    pub(crate) fn time<R>(&mut self, op: impl FnOnce() -> R) -> R {
        let (bytes_before, calls_before) = allocation_counters();
        let started = Instant::now();
        let out = op();
        self.elapsed += started.elapsed();
        let (bytes_after, calls_after) = allocation_counters();
        self.bytes += bytes_after - bytes_before;
        self.calls += calls_after - calls_before;
        out
    }
}

pub(crate) fn measure_segments(
    name: &'static str,
    note: &'static str,
    warmup: usize,
    iters: usize,
    mut op: impl FnMut(&mut SegmentTimer),
) -> Metric {
    for _ in 0..warmup {
        op(&mut SegmentTimer::default());
    }
    let mut samples = Vec::with_capacity(iters);
    let mut total = Duration::ZERO;
    let mut bytes = 0u64;
    let mut calls = 0u64;
    for _ in 0..iters {
        let mut timer = SegmentTimer::default();
        op(&mut timer);
        samples.push(timer.elapsed);
        total += timer.elapsed;
        bytes += timer.bytes;
        calls += timer.calls;
    }
    from_samples(name, note, &mut samples, total, (bytes, calls), iters)
}

pub(crate) fn env_or(name: &str, fallback: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| fallback.to_owned())
}

pub(crate) const SCHEMA: u64 = 2;

pub(crate) fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

pub(crate) fn baselines_dir() -> PathBuf {
    std::env::var_os("PANEFLOW_BENCH_BASELINE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../bench/baselines"))
}

pub(crate) fn baseline_path(name: &str) -> PathBuf {
    baselines_dir()
        .join(platform())
        .join(format!("{name}.json"))
}

fn recorded_platform(document: &serde_json::Value) -> String {
    format!(
        "{}-{}",
        document["os"].as_str().unwrap_or("unknown"),
        document["arch"].as_str().unwrap_or("unknown")
    )
}

fn comparable_baseline(path: &Path) -> Result<serde_json::Value, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("No baseline for {}.", platform()));
        }
        Err(error) => {
            return Err(format!(
                "Baseline {} is unreadable ({error}); no comparison.",
                path.display()
            ));
        }
    };
    let baseline: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
        format!(
            "Baseline {} is not JSON ({error}); no comparison.",
            path.display()
        )
    })?;
    let recorded = recorded_platform(&baseline);
    if recorded != platform() {
        return Err(format!(
            "Baseline {} was recorded on {recorded}, not {}; no comparison.",
            path.display(),
            platform()
        ));
    }
    if baseline["schema"].as_u64() != Some(SCHEMA) {
        return Err(format!(
            "Baseline {} has schema {}, this run writes schema {SCHEMA}; no comparison, record a new baseline.",
            path.display(),
            baseline["schema"]
        ));
    }
    Ok(baseline)
}

pub(crate) fn baseline_violations(
    path: &Path,
    directory: &str,
    document: &serde_json::Value,
) -> Vec<String> {
    let mut violations = Vec::new();
    if document["schema"].as_u64() != Some(SCHEMA) {
        violations.push(format!(
            "{}: schema {}, expected schema {SCHEMA}",
            path.display(),
            document["schema"]
        ));
    }
    if document["git_dirty"].as_str() != Some("false") {
        violations.push(format!(
            "{}: git_dirty is {}, a baseline must come from a clean tree",
            path.display(),
            document["git_dirty"]
        ));
    }
    let recorded = recorded_platform(document);
    if recorded != directory || document["platform"].as_str() != Some(directory) {
        violations.push(format!(
            "{}: recorded on {recorded} (platform {}), but it lives under {directory}",
            path.display(),
            document["platform"]
        ));
    }
    violations
}

fn document(suite: &str, corpus_seed: u64, metrics: &[Metric]) -> serde_json::Value {
    let generated_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    serde_json::json!({
        "schema": SCHEMA,
        "suite": suite,
        "generated_unix": generated_unix,
        "stamp": env_or("PANEFLOW_BENCH_STAMP", "unknown"),
        "git_sha": env_or("PANEFLOW_BENCH_SHA", "unknown"),
        "git_dirty": env_or("PANEFLOW_BENCH_DIRTY", "unknown"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "platform": platform(),
        "cpu": cpu_model(),
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "corpus_seed": format!("0x{corpus_seed:016x}"),
        "metrics": metrics.iter().map(Metric::to_json).collect::<Vec<_>>(),
    })
}

pub(crate) fn format_value(value: f64, unit: &str) -> String {
    match unit {
        "ns" if value >= 1_000_000.0 => format!("{:.2} ms", value / 1_000_000.0),
        "ns" if value >= 1_000.0 => format!("{:.1} us", value / 1_000.0),
        "ns" => format!("{value:.0} ns"),
        "MiB/s" => format!("{value:.1} MiB/s"),
        "bytes" => format_bytes(value),
        _ => format!("{value:.0} {unit}"),
    }
}

pub(crate) fn format_bytes(value: f64) -> String {
    if value >= 1024.0 * 1024.0 {
        format!("{:.2} MiB", value / (1024.0 * 1024.0))
    } else if value >= 1024.0 {
        format!("{:.1} KiB", value / 1024.0)
    } else {
        format!("{value:.0} B")
    }
}

fn run_header() -> String {
    format!(
        "Run `{}` ({}), {} {} on {}.\n\n",
        env_or("PANEFLOW_BENCH_SHA", "unknown"),
        env_or("PANEFLOW_BENCH_STAMP", "unknown"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        cpu_model(),
    )
}

pub(crate) fn results_table(current: &[Metric], no_comparison: &str) -> String {
    let mut table = run_header();
    table.push_str(no_comparison);
    table.push_str("\n\n");
    table.push_str("| Metric | Now | Alloc/iter now |\n");
    table.push_str("|---|---|---|\n");
    for metric in current {
        let value = if metric.available {
            format_value(metric.value, metric.unit)
        } else {
            "unavailable".to_owned()
        };
        table.push_str(&format!(
            "| `{}` | {} | {} |\n",
            metric.name,
            value,
            metric
                .alloc_bytes_per_iter
                .map(format_bytes)
                .unwrap_or_else(|| "n/a".into()),
        ));
    }
    table
}

pub(crate) fn comparison_table(current: &[Metric], baseline: &serde_json::Value) -> String {
    let baseline_metrics = baseline
        .get("metrics")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let find = |name: &str| {
        baseline_metrics
            .iter()
            .find(|metric| metric.get("metric").and_then(serde_json::Value::as_str) == Some(name))
    };
    let mut table = String::new();
    table.push_str(&format!(
        "Baseline `{}` ({}) versus `{}` ({}), {} {} on {}.\n\n",
        baseline
            .get("git_sha")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown"),
        baseline
            .get("stamp")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown"),
        env_or("PANEFLOW_BENCH_SHA", "unknown"),
        env_or("PANEFLOW_BENCH_STAMP", "unknown"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        cpu_model(),
    ));
    table.push_str("| Metric | Baseline | Now | Change | Alloc/iter baseline | Alloc/iter now |\n");
    table.push_str("|---|---|---|---|---|---|\n");
    for metric in current {
        let Some(previous) = find(metric.name) else {
            let value = if metric.available {
                format_value(metric.value, metric.unit)
            } else {
                "unavailable".to_owned()
            };
            table.push_str(&format!(
                "| `{}` | n/a | {} | new | n/a | {} |\n",
                metric.name,
                value,
                metric
                    .alloc_bytes_per_iter
                    .map(format_bytes)
                    .unwrap_or_else(|| "n/a".into()),
            ));
            continue;
        };
        let before = previous.get("value").and_then(serde_json::Value::as_f64);
        let before_alloc = previous
            .get("alloc_bytes_per_iter")
            .and_then(serde_json::Value::as_f64);
        let change = if metric.available && before.is_some_and(|value| value > 0.0) {
            let before = before.expect("a positive baseline value exists");
            let ratio = match metric.direction {
                Direction::LowerIsBetter => before / metric.value,
                Direction::HigherIsBetter => metric.value / before,
            };
            let percent = (metric.value - before) / before * 100.0;
            format!("{percent:+.1}% ({ratio:.2}x)")
        } else {
            "n/a".to_owned()
        };
        let before_value = before
            .map(|value| format_value(value, metric.unit))
            .unwrap_or_else(|| "unavailable".to_owned());
        let current_value = if metric.available {
            format_value(metric.value, metric.unit)
        } else {
            "unavailable".to_owned()
        };
        table.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} |\n",
            metric.name,
            before_value,
            current_value,
            change,
            before_alloc
                .map(format_bytes)
                .unwrap_or_else(|| "n/a".into()),
            metric
                .alloc_bytes_per_iter
                .map(format_bytes)
                .unwrap_or_else(|| "n/a".into()),
        ));
    }
    table
}

fn samples_document(suite: &str, metrics: &[Metric]) -> serde_json::Value {
    let sampled: serde_json::Map<String, serde_json::Value> = metrics
        .iter()
        .filter(|metric| metric.available && !metric.samples_ns.is_empty())
        .map(|metric| {
            (
                metric.name.to_string(),
                serde_json::json!({"unit": metric.unit, "samples": metric.samples_ns}),
            )
        })
        .collect();
    serde_json::json!({
        "suite": suite,
        "git_sha": env_or("PANEFLOW_BENCH_SHA", "unknown"),
        "metrics": sampled,
    })
}

pub(crate) fn publish(
    suite: &str,
    baseline: &str,
    corpus_seed: u64,
    metrics: &[Metric],
    cpu_share: Option<f64>,
) {
    for metric in metrics {
        println!("PANEFLOW_BENCH_METRIC {}", metric.to_json());
    }
    let mut document = document(suite, corpus_seed, metrics);
    document["cpu_share"] = serde_json::json!(cpu_share);
    println!("PANEFLOW_BENCH_DOCUMENT {document}");

    if let Some(path) = std::env::var_os("PANEFLOW_BENCH_OUT") {
        let pretty = serde_json::to_string_pretty(&document).expect("document serializes");
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&path, pretty).expect("benchmark output must be writable");
        println!("PANEFLOW_BENCH_WRITTEN {}", path.to_string_lossy());
    }
    if let Some(path) = std::env::var_os("PANEFLOW_BENCH_SAMPLES_OUT") {
        std::fs::write(&path, samples_document(suite, metrics).to_string())
            .expect("the sample file must be writable");
        println!("PANEFLOW_BENCH_SAMPLES_WRITTEN {}", path.to_string_lossy());
    }

    let baseline_path = baseline_path(baseline);
    println!("PANEFLOW_BENCH_BASELINE {}", baseline_path.display());
    println!("PANEFLOW_BENCH_TABLE_BEGIN");
    match comparable_baseline(&baseline_path) {
        Ok(baseline) => print!("{}", comparison_table(metrics, &baseline)),
        Err(no_comparison) => print!("{}", results_table(metrics, &no_comparison)),
    }
    println!("PANEFLOW_BENCH_TABLE_END");
}

pub(crate) fn percentile_duration(values: &[Duration], percentile: usize) -> Duration {
    let index = values.len().saturating_sub(1).saturating_mul(percentile) / 100;
    values.get(index).copied().unwrap_or_default()
}

pub(crate) fn percentile_us(values: &[Duration], percentile: usize) -> u128 {
    percentile_duration(values, percentile).as_micros()
}

#[cfg(target_os = "linux")]
pub(crate) fn resident_set_bytes() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let resident_pages = statm
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(0) as u64;
    resident_pages.saturating_mul(page_size)
}

#[cfg(target_os = "windows")]
pub(crate) fn resident_set_bytes() -> u64 {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut memory: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    memory.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let result = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut memory,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
    };
    if result == 0 {
        return 0;
    }
    u64::try_from(memory.WorkingSetSize).unwrap_or(u64::MAX)
}

#[cfg(target_os = "macos")]
fn current_task_info() -> Option<libc::proc_taskinfo> {
    let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    let written = unsafe {
        libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDTASKINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    (written == size).then_some(info)
}

#[cfg(target_os = "macos")]
pub(crate) fn resident_set_bytes() -> u64 {
    current_task_info().map_or(0, |info| info.pti_resident_size)
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
pub(crate) fn resident_set_bytes() -> u64 {
    0
}

#[cfg(target_os = "linux")]
pub(crate) fn process_cpu_time() -> Duration {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let fields = stat
        .rsplit_once(')')
        .map(|(_, fields)| fields)
        .unwrap_or("");
    let mut values = fields.split_whitespace();
    let user_ticks = values
        .nth(11)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let system_ticks = values
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let ticks_per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
    Duration::from_secs_f64((user_ticks + system_ticks) as f64 / ticks_per_second as f64)
}

#[cfg(target_os = "windows")]
pub(crate) fn process_cpu_time() -> Duration {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    let mut creation: FILETIME = unsafe { std::mem::zeroed() };
    let mut exit: FILETIME = unsafe { std::mem::zeroed() };
    let mut kernel: FILETIME = unsafe { std::mem::zeroed() };
    let mut user: FILETIME = unsafe { std::mem::zeroed() };
    let result = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    };
    if result == 0 {
        return Duration::ZERO;
    }
    let ticks =
        |value: FILETIME| (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime);
    Duration::from_nanos(
        ticks(kernel)
            .saturating_add(ticks(user))
            .saturating_mul(100),
    )
}

#[cfg(target_os = "macos")]
#[allow(
    deprecated,
    reason = "libc deprecates mach_timebase_info in favor of the mach2 crate, which is not a dependency"
)]
pub(crate) fn process_cpu_time() -> Duration {
    let Some(info) = current_task_info() else {
        return Duration::ZERO;
    };
    let mut timebase = libc::mach_timebase_info { numer: 0, denom: 0 };
    if unsafe { libc::mach_timebase_info(&mut timebase) } != 0 || timebase.denom == 0 {
        return Duration::ZERO;
    }
    let ticks = info.pti_total_user.saturating_add(info.pti_total_system);
    Duration::from_nanos(mach_ticks_to_nanos(ticks, timebase.numer, timebase.denom))
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
pub(crate) fn process_cpu_time() -> Duration {
    Duration::ZERO
}

fn mach_ticks_to_nanos(ticks: u64, numer: u32, denom: u32) -> u64 {
    let nanos = u128::from(ticks) * u128::from(numer) / u128::from(denom.max(1));
    u64::try_from(nanos).unwrap_or(u64::MAX)
}

#[cfg(target_os = "linux")]
pub(crate) fn cpu_model() -> String {
    std::fs::read_to_string("/proc/cpuinfo")
        .unwrap_or_default()
        .lines()
        .find_map(|line| line.strip_prefix("model name\t: "))
        .unwrap_or("unknown")
        .to_owned()
}

#[cfg(target_os = "macos")]
pub(crate) fn cpu_model() -> String {
    std::process::Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|model| !model.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

#[cfg(target_os = "windows")]
pub(crate) fn cpu_model() -> String {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};

    let wide = |text: &str| text.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let key = wide("HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0");
    let value = wide("ProcessorNameString");
    let mut buffer = [0u16; 256];
    let mut bytes = std::mem::size_of_val(&buffer) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if status != ERROR_SUCCESS {
        return "unknown".to_owned();
    }
    let units = (bytes as usize / 2).min(buffer.len());
    let model = String::from_utf16_lossy(&buffer[..units]);
    let model = model.trim_end_matches('\0').trim();
    if model.is_empty() {
        "unknown".to_owned()
    } else {
        model.to_owned()
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub(crate) fn cpu_model() -> String {
    "unknown".to_owned()
}

#[allow(
    clippy::assertions_on_constants,
    reason = "a benchmark refuses a debug-profile run unless asked to allow it"
)]
pub(crate) fn refuse_debug_profile() {
    assert!(
        !cfg!(debug_assertions) || std::env::var_os("PANEFLOW_BENCH_ALLOW_DEBUG").is_some(),
        "run this benchmark with cargo test --release (or set PANEFLOW_BENCH_ALLOW_DEBUG=1)"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mach_ticks_convert_through_the_timebase() {
        assert_eq!(mach_ticks_to_nanos(1_000, 1, 1), 1_000);
        assert_eq!(mach_ticks_to_nanos(24_000_000, 125, 3), 1_000_000_000);
        assert_eq!(mach_ticks_to_nanos(u64::MAX, 125, 3), u64::MAX);
        assert_eq!(mach_ticks_to_nanos(7, 1, 0), 7);
    }

    #[test]
    fn this_platform_reports_resident_memory_and_cpu_time() {
        let start = process_cpu_time();
        let deadline = std::time::Instant::now() + Duration::from_millis(50);
        let mut spin = 0_u64;
        while std::time::Instant::now() < deadline {
            spin = std::hint::black_box(spin.wrapping_add(1));
        }
        assert!(resident_set_bytes() > 0);
        assert!(process_cpu_time() > start);
    }

    #[test]
    fn only_timed_available_metrics_export_their_raw_samples_in_measurement_order() {
        let mut iteration = 0u64;
        let timed = measure("timed", "", 0, 60, || {
            iteration += 1;
            if iteration == 1 {
                std::thread::sleep(Duration::from_millis(2));
            }
        });
        let metrics = [
            timed,
            Metric::count("counted", "frames", 3.0, ""),
            Metric::unavailable("missing", "ns", ""),
        ];
        let document = samples_document("suite", &metrics);
        let exported = document["metrics"].as_object().unwrap();
        assert_eq!(exported.keys().collect::<Vec<_>>(), ["timed"]);
        assert_eq!(exported["timed"]["unit"], "ns");
        let samples = exported["timed"]["samples"].as_array().unwrap();
        assert_eq!(samples.len(), 60);
        assert!(samples[0].as_f64().unwrap() >= 2_000_000.0);
        assert!(samples[59].as_f64().unwrap() < 2_000_000.0);
    }

    #[test]
    fn comparison_table_reports_speedups_from_the_baseline() {
        let now = [Metric {
            name: "publish_scroll_220x60",
            unit: "ns",
            direction: Direction::LowerIsBetter,
            value: 500_000.0,
            p95: None,
            p99: None,
            mean: None,
            alloc_bytes_per_iter: Some(1024.0),
            allocs_per_iter: Some(1.0),
            iters: 1,
            note: "",
            available: true,
            samples_ns: Vec::new(),
        }];
        let baseline = serde_json::json!({
            "git_sha": "abc",
            "stamp": "t0",
            "metrics": [{
                "metric": "publish_scroll_220x60",
                "value": 1_000_000.0,
                "alloc_bytes_per_iter": 2048.0
            }]
        });
        let table = comparison_table(&now, &baseline);
        assert!(table.contains("| `publish_scroll_220x60` | 1.00 ms | 500.0 us | -50.0% (2.00x) | 2.0 KiB | 1.0 KiB |"), "{table}");
    }

    #[test]
    fn results_table_drops_the_comparison_columns_without_a_baseline() {
        let now = [Metric::count("open_300kb_highlighted", "ns", 1_500.0, "")];
        let table = results_table(&now, "No baseline for macos-aarch64.");
        assert!(table.contains("No baseline for macos-aarch64."), "{table}");
        assert!(
            table.contains("| Metric | Now | Alloc/iter now |"),
            "{table}"
        );
        assert!(!table.contains("Change"), "{table}");
        assert!(
            table.contains("| `open_300kb_highlighted` | 1.5 us | n/a |"),
            "{table}"
        );
    }

    #[test]
    fn unavailable_metrics_remain_in_the_document_and_tables() {
        let metric = Metric::unavailable("shape_cold_60_rows", "ns", "no platform text system");
        let json = metric.to_json();
        assert_eq!(json["available"], false);
        assert!(json["value"].is_null());
        assert!(
            results_table(&[metric], "No baseline for linux-x86_64.")
                .contains("| `shape_cold_60_rows` | unavailable | n/a |")
        );
    }

    const HARNESS_BASELINES: [&str; 5] = [
        "terminal",
        "editor",
        "startup",
        "terminal-alloc",
        "editor-alloc",
    ];
    const PERSISTENT_BASELINES: [&str; 2] = ["persistent", "persistent-active"];

    fn committed_baselines() -> Vec<(String, PathBuf)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../bench/baselines");
        let mut files = Vec::new();
        for directory in std::fs::read_dir(&root).expect("bench/baselines exists") {
            let directory = directory.expect("bench/baselines is listable").path();
            let platform = directory
                .file_name()
                .and_then(|name| name.to_str())
                .expect("a platform directory has a UTF-8 name")
                .to_owned();
            for file in std::fs::read_dir(&directory).expect("a platform directory is listable") {
                files.push((
                    platform.clone(),
                    file.expect("a baseline is listable").path(),
                ));
            }
        }
        files.sort();
        files
    }

    #[test]
    fn every_committed_baseline_is_clean_current_and_on_its_platform() {
        let mut checked = 0;
        let mut violations = Vec::new();
        for (platform, path) in committed_baselines() {
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("");
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                violations.push(format!("{}: not a JSON baseline", path.display()));
                continue;
            }
            if PERSISTENT_BASELINES.contains(&stem) {
                continue;
            }
            if !HARNESS_BASELINES.contains(&stem) {
                violations.push(format!(
                    "{}: unknown baseline, expected one of {HARNESS_BASELINES:?} or {PERSISTENT_BASELINES:?}",
                    path.display()
                ));
                continue;
            }
            let document: serde_json::Value = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or_else(|| panic!("{} is not readable JSON", path.display()));
            violations.extend(baseline_violations(&path, &platform, &document));
            checked += 1;
        }
        assert!(
            checked > 0,
            "no harness baseline is committed under bench/baselines"
        );
        assert!(
            violations.is_empty(),
            "committed baselines must be at schema {SCHEMA}, from a clean tree, under their own platform; record them again with scripts/bench-<suite>.sh --set-baseline or scripts/perf-gates.sh --refresh-alloc-baselines:\n{}",
            violations.join("\n")
        );
    }

    #[test]
    fn an_old_schema_baseline_fails_the_coherence_check_naming_the_file_and_the_schema() {
        let path = Path::new("bench/baselines/linux-x86_64/terminal.json");
        let old = serde_json::json!({
            "schema": 1,
            "git_dirty": "false",
            "os": "linux",
            "arch": "x86_64",
            "platform": "linux-x86_64",
        });
        let violations = baseline_violations(path, "linux-x86_64", &old);
        assert_eq!(
            violations,
            [format!(
                "bench/baselines/linux-x86_64/terminal.json: schema 1, expected schema {SCHEMA}"
            )]
        );
    }

    #[test]
    fn a_dirty_or_misplaced_baseline_fails_the_coherence_check() {
        let path = Path::new("bench/baselines/linux-x86_64/editor.json");
        let windows = serde_json::json!({
            "schema": SCHEMA,
            "git_dirty": "true",
            "os": "windows",
            "arch": "x86_64",
            "platform": "windows-x86_64",
        });
        let violations = baseline_violations(path, "linux-x86_64", &windows);
        assert_eq!(violations.len(), 2, "{violations:?}");
        assert!(
            violations[0].contains("git_dirty is \"true\""),
            "{violations:?}"
        );
        assert!(
            violations[1].contains("recorded on windows-x86_64"),
            "{violations:?}"
        );
    }

    #[test]
    fn a_missing_baseline_names_the_current_platform_and_compares_nothing() {
        let home = tempfile::tempdir().unwrap();
        let refusal = comparable_baseline(&home.path().join("terminal.json")).unwrap_err();
        assert_eq!(refusal, format!("No baseline for {}.", platform()));
    }

    #[test]
    fn a_baseline_from_another_platform_or_schema_is_never_compared() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("terminal.json");
        let foreign = serde_json::json!({
            "schema": SCHEMA, "os": "plan9", "arch": "mips", "metrics": [],
        });
        std::fs::write(&path, foreign.to_string()).unwrap();
        let refusal = comparable_baseline(&path).unwrap_err();
        assert!(
            refusal.contains(&format!("was recorded on plan9-mips, not {}", platform())),
            "{refusal}"
        );
        let old = serde_json::json!({
            "schema": 1,
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "metrics": [],
        });
        std::fs::write(&path, old.to_string()).unwrap();
        let refusal = comparable_baseline(&path).unwrap_err();
        assert!(
            refusal.contains(&format!("has schema 1, this run writes schema {SCHEMA}")),
            "{refusal}"
        );
        let current = serde_json::json!({
            "schema": SCHEMA,
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "metrics": [],
        });
        std::fs::write(&path, current.to_string()).unwrap();
        assert!(comparable_baseline(&path).is_ok());
    }

    #[test]
    fn this_platform_reports_a_real_cpu_model() {
        let model = cpu_model();
        assert!(!model.is_empty());
        assert_ne!(
            model,
            format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
        );
    }

    #[test]
    fn live_bytes_tracks_a_retained_allocation() {
        const RETAINED_BYTES: usize = 16 * 1024 * 1024;
        const CONCURRENT_NOISE_MARGIN: i64 = (RETAINED_BYTES / 2) as i64;
        let before = live_bytes();
        let retained = vec![0u8; RETAINED_BYTES];
        let during = live_bytes();
        assert!(
            during - before >= CONCURRENT_NOISE_MARGIN,
            "a 16 MiB vector must dominate concurrent allocator noise: {before} -> {during}"
        );
        drop(retained);
        let after = live_bytes();
        assert!(
            during - after >= CONCURRENT_NOISE_MARGIN,
            "dropping a 16 MiB vector must dominate concurrent allocator noise: {during} -> {after}"
        );
    }
}
