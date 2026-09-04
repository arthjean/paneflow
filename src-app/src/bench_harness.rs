use std::alloc::{GlobalAlloc, Layout, System};
use std::ffi::c_void;
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

pub(crate) fn count_tree_sitter_allocations() {
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
    pub(crate) mean: Option<f64>,
    pub(crate) alloc_bytes_per_iter: Option<f64>,
    pub(crate) allocs_per_iter: Option<f64>,
    pub(crate) iters: usize,
    pub(crate) note: &'static str,
    pub(crate) available: bool,
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
            mean: None,
            alloc_bytes_per_iter: None,
            allocs_per_iter: None,
            iters: 1,
            note,
            available: true,
        }
    }

    pub(crate) fn unavailable(name: &'static str, unit: &'static str, note: &'static str) -> Self {
        Self {
            name,
            unit,
            direction: Direction::LowerIsBetter,
            value: 0.0,
            p95: None,
            mean: None,
            alloc_bytes_per_iter: None,
            allocs_per_iter: None,
            iters: 0,
            note,
            available: false,
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
    samples.sort_unstable();
    let iters_f = iters.max(1) as f64;
    Metric {
        name,
        unit: "ns",
        direction: Direction::LowerIsBetter,
        value: percentile_duration(samples, 50).as_nanos() as f64,
        p95: Some(percentile_duration(samples, 95).as_nanos() as f64),
        mean: Some(total.as_nanos() as f64 / iters_f),
        alloc_bytes_per_iter: Some(allocated.0 as f64 / iters_f),
        allocs_per_iter: Some(allocated.1 as f64 / iters_f),
        iters,
        note,
        available: true,
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

fn document(suite: &str, corpus_seed: u64, metrics: &[Metric]) -> serde_json::Value {
    let generated_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    serde_json::json!({
        "schema": 1,
        "suite": suite,
        "generated_unix": generated_unix,
        "stamp": env_or("PANEFLOW_BENCH_STAMP", "unknown"),
        "git_sha": env_or("PANEFLOW_BENCH_SHA", "unknown"),
        "git_dirty": env_or("PANEFLOW_BENCH_DIRTY", "unknown"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
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

pub(crate) fn results_table(current: &[Metric]) -> String {
    let mut table = run_header();
    table.push_str("No baseline to compare against.\n\n");
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

pub(crate) fn publish(suite: &str, corpus_seed: u64, metrics: &[Metric], cpu_share: f64) {
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

    let baseline = std::env::var_os("PANEFLOW_BENCH_BASELINE")
        .and_then(|path| std::fs::read_to_string(&path).ok())
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    println!("PANEFLOW_BENCH_TABLE_BEGIN");
    match baseline {
        Some(baseline) => print!("{}", comparison_table(metrics, &baseline)),
        None => print!("{}", results_table(metrics)),
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

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
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

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub(crate) fn process_cpu_time() -> Duration {
    Duration::ZERO
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

#[cfg(not(target_os = "linux"))]
pub(crate) fn cpu_model() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
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
    fn comparison_table_reports_speedups_from_the_baseline() {
        let now = [Metric {
            name: "publish_scroll_220x60",
            unit: "ns",
            direction: Direction::LowerIsBetter,
            value: 500_000.0,
            p95: None,
            mean: None,
            alloc_bytes_per_iter: Some(1024.0),
            allocs_per_iter: Some(1.0),
            iters: 1,
            note: "",
            available: true,
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
        let table = results_table(&now);
        assert!(table.contains("No baseline to compare against."), "{table}");
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
        assert!(results_table(&[metric]).contains("| `shape_cold_60_rows` | unavailable | n/a |"));
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
