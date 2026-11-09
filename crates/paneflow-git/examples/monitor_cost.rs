use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use paneflow_git::feed::FeedOptions;
use paneflow_git::{snapshot, Freshness, Monitor, Options, ProbeKind, ProbeReport, Snapshot};

#[path = "support/process.rs"]
mod process;

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

mod heap {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static CURRENT: AtomicUsize = AtomicUsize::new(0);

    static PEAK: AtomicUsize = AtomicUsize::new(0);

    pub(crate) struct Counting;

    fn grow(bytes: usize) {
        let now = CURRENT.fetch_add(bytes, Ordering::Relaxed) + bytes;
        PEAK.fetch_max(now, Ordering::Relaxed);
    }

    fn shrink(bytes: usize) {
        CURRENT.fetch_sub(bytes, Ordering::Relaxed);
    }

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let pointer = unsafe { System.alloc(layout) };
            if !pointer.is_null() {
                grow(layout.size());
            }
            pointer
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let pointer = unsafe { System.alloc_zeroed(layout) };
            if !pointer.is_null() {
                grow(layout.size());
            }
            pointer
        }

        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            unsafe { System.dealloc(pointer, layout) };
            shrink(layout.size());
        }

        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            let moved = unsafe { System.realloc(pointer, layout, size) };
            if !moved.is_null() {
                if size >= layout.size() {
                    grow(size - layout.size());
                } else {
                    shrink(layout.size() - size);
                }
            }
            moved
        }
    }

    pub(crate) fn current() -> usize {
        CURRENT.load(Ordering::Relaxed)
    }

    pub(crate) fn peak() -> usize {
        PEAK.load(Ordering::Relaxed)
    }

    pub(crate) fn reset_peak() {
        PEAK.store(current(), Ordering::Relaxed);
    }
}

const CLI_ARGS: &[&str] = &[
    "--no-optional-locks",
    "status",
    "--porcelain=v1",
    "-z",
    "-uall",
    "--no-renames",
];

const USAGE: &str = "usage: monitor_cost <repository> [iterations] [git-iterations]\n       monitor_cost <repository> --heap <monitor|full> [iterations]";

const DEFAULT_ITERATIONS: usize = 40;

const DEFAULT_GIT_ITERATIONS: usize = 10;

const SETTLE: Duration = Duration::from_millis(150);

const CONSUMERS: usize = 8;

const EDITS: usize = 100;

const STAGED: usize = 3;

const ENGINES: usize = 3;

const PRELOAD_ENV: &str = "LD_PRELOAD";

const INPUT_NAMES: [&str; 3] = [".gitignore", ".gitattributes", ".gitmodules"];

const IDENTITY: [&str; 4] = [
    "-c",
    "user.name=monitor-cost",
    "-c",
    "user.email=monitor-cost@localhost",
];

#[derive(Clone, Copy)]
struct Cost {
    wall: Duration,
    cpu: Duration,
}

#[derive(Default)]
struct Series {
    monitor: Vec<Cost>,
    full: Vec<Cost>,
    cli: Vec<Cost>,
    kinds: BTreeMap<String, usize>,
    rechecked: Vec<usize>,
    hashed: Vec<usize>,
    stat_calls: Vec<usize>,
    mismatches: usize,
}

struct Bench {
    _scratch: tempfile::TempDir,
    work: PathBuf,
    monitor: Mutex<Monitor>,
    options: Options,
    edits: Vec<String>,
    originals: BTreeMap<String, Vec<u8>>,
    revision: usize,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(repository) = args.first().map(PathBuf::from) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let result = if args.get(1).map(String::as_str) == Some("--heap") {
        let iterations = number(args.get(3), DEFAULT_ITERATIONS);
        match args.get(2).map(String::as_str) {
            Some("monitor") => heap(&repository, true, iterations),
            Some("full") => heap(&repository, false, iterations),
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    } else {
        run(
            &repository,
            number(args.get(1), DEFAULT_ITERATIONS),
            number(args.get(2), DEFAULT_GIT_ITERATIONS),
        )
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn number(value: Option<&String>, default: usize) -> usize {
    value
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn run(repository: &Path, iterations: usize, git_iterations: usize) -> Result<(), String> {
    let mut bench = Bench::prepare(repository)?;
    let baseline = bench.baseline()?;
    println!(
        "repository={} entries={} baseline_wall_ms={:.2} baseline_cpu_ms={:.2}{}",
        repository.display(),
        bench.entries()?,
        millis(baseline.wall),
        millis(baseline.cpu),
        bench.watch_counts()
    );
    let mut results = Vec::new();
    results.push(("no_change", bench.series(iterations, |_| Ok(()))?));
    results.push(("one_edit", bench.series(iterations, |bench| bench.edit(1))?));
    bench.restore()?;
    results.push((
        "hundred_edits",
        bench.series(iterations, |bench| bench.edit(EDITS))?,
    ));
    bench.restore()?;
    results.push(("git_add", bench.git_add(git_iterations)?));
    results.push(("checkout", bench.checkout(git_iterations)?));
    results.push(("git_commit", bench.git_commit(git_iterations)?));
    results.push(("eight_consumers", bench.concurrent(iterations)?));
    for (scenario, series) in &results {
        report(scenario, series);
    }
    let peak = process::self_peak_bytes();
    println!("process_peak_mib={:.1}", mebibytes(peak));
    Ok(())
}

fn heap(repository: &Path, monitored: bool, iterations: usize) -> Result<(), String> {
    let mut bench = Bench::prepare(repository)?;
    heap::reset_peak();
    let start = heap::current();
    if monitored {
        bench.baseline()?;
    }
    let retained = heap::current().saturating_sub(start);
    let baseline_peak = heap::peak().saturating_sub(start);
    for _ in 0..iterations {
        bench.edit(1)?;
        if monitored {
            bench.probe_monitor()?;
        } else {
            bench.probe_full()?;
        }
    }
    println!(
        "heap_mode={} iterations={iterations} heap_peak_mib={:.2} baseline_peak_mib={:.2} retained_after_baseline_mib={:.2} retained_at_end_mib={:.2} process_peak_mib={:.1}",
        if monitored { "monitor" } else { "full" },
        mebibytes_usize(heap::peak().saturating_sub(start)),
        mebibytes_usize(baseline_peak),
        mebibytes_usize(retained),
        mebibytes_usize(heap::current().saturating_sub(start)),
        mebibytes(process::self_peak_bytes()),
    );
    Ok(())
}

fn mebibytes_usize(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

impl Bench {
    fn prepare(repository: &Path) -> Result<Self, String> {
        let head = git_output(repository, &["rev-parse", "HEAD"])?;
        let scratch = tempfile::tempdir().map_err(|error| format!("scratch: {error}"))?;
        let work = scratch.path().join("work");
        let source = repository
            .to_str()
            .ok_or_else(|| format!("{} is not UTF-8", repository.display()))?;
        let target = work
            .to_str()
            .ok_or_else(|| format!("{} is not UTF-8", work.display()))?;
        git(
            scratch.path(),
            &["clone", "-q", "--shared", "--no-checkout", source, target],
        )?;
        git(&work, &["checkout", "-q", "--detach", head.trim()])?;
        git(&work, &["status", "--porcelain"])?;
        let work = std::fs::canonicalize(&work).map_err(|error| format!("canonical: {error}"))?;
        let edits = spread_files(&work, EDITS)?;
        let mut originals = BTreeMap::new();
        for file in &edits {
            let content =
                std::fs::read(work.join(file)).map_err(|error| format!("{file}: {error}"))?;
            originals.insert(file.clone(), content);
        }
        let cache = scratch.path().join("cache");
        let monitor = Monitor::configured(
            &work,
            FeedOptions {
                cache_dir: Some(cache),
                ..FeedOptions::default()
            },
            true,
        );
        Ok(Self {
            _scratch: scratch,
            work,
            monitor: Mutex::new(monitor),
            options: Options::files_status(),
            edits,
            originals,
            revision: 0,
        })
    }

    fn monitor(&self) -> std::sync::MutexGuard<'_, Monitor> {
        self.monitor.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn baseline(&mut self) -> Result<Cost, String> {
        let (cost, _, report) = self.probe_monitor()?;
        if !matches!(report.kind, ProbeKind::Baseline(_)) {
            return Err(format!(
                "the first probe was not a baseline: {:?}",
                report.kind
            ));
        }
        Ok(cost)
    }

    fn entries(&self) -> Result<usize, String> {
        Ok(git_output(&self.work, &["ls-files"])?.lines().count())
    }

    fn watch_counts(&self) -> String {
        let watches = self.monitor().feed().map_or(0, |feed| feed.watches().len());
        #[cfg(target_os = "linux")]
        let inotify = format!(
            " inotify_watches_in_use={}",
            paneflow_git::feed::inotify_watches_in_use()
        );
        #[cfg(not(target_os = "linux"))]
        let inotify = String::new();
        format!(" feed_watches={watches}{inotify}")
    }

    fn probe_monitor(&self) -> Result<(Cost, Snapshot, ProbeReport), String> {
        let mut monitor = self.monitor();
        let cpu = process::self_cpu();
        let started = Instant::now();
        let snapshot = monitor
            .probe(&self.work, &self.options, Freshness::Converging)
            .map_err(|error| format!("monitor probe: {error}"))?
            .into_snapshot();
        let wall = started.elapsed();
        let cpu = process::self_cpu().saturating_sub(cpu);
        Ok((Cost { wall, cpu }, snapshot, monitor.last_probe().clone()))
    }

    fn probe_full(&self) -> Result<(Cost, Snapshot), String> {
        let cpu = process::self_cpu();
        let started = Instant::now();
        let snapshot =
            snapshot(&self.work, &self.options).map_err(|error| format!("full: {error}"))?;
        Ok((
            Cost {
                wall: started.elapsed(),
                cpu: process::self_cpu().saturating_sub(cpu),
            },
            snapshot,
        ))
    }

    fn probe_cli(&self) -> Result<Cost, String> {
        let mut command = Command::new("git");
        command
            .args(CLI_ARGS)
            .current_dir(&self.work)
            .env_remove(PRELOAD_ENV);
        let started = Instant::now();
        let child = process::run(command).map_err(|error| format!("git status: {error}"))?;
        let wall = started.elapsed();
        if !child.success {
            return Err(format!(
                "git status failed: {}",
                String::from_utf8_lossy(&child.stderr)
            ));
        }
        Ok(Cost {
            wall,
            cpu: child.cpu,
        })
    }

    fn measure(&self, series: &mut Series) -> Result<(), String> {
        let turn = series.monitor.len() % ENGINES;
        let mut probed = None;
        let mut full = None;
        for step in 0..ENGINES {
            match (turn + step) % ENGINES {
                0 => {
                    let (cost, snapshot, report) = self.probe_monitor()?;
                    series.monitor.push(cost);
                    *series.kinds.entry(kind_name(&report.kind)).or_default() += 1;
                    series.rechecked.push(report.rechecked_entries);
                    series.hashed.push(report.files_hashed);
                    series.stat_calls.push(report.symlink_metadata_calls);
                    probed = Some(snapshot);
                }
                1 => {
                    let (cost, snapshot) = self.probe_full()?;
                    series.full.push(cost);
                    full = Some(snapshot);
                }
                _ => series.cli.push(self.probe_cli()?),
            }
        }
        if let (Some(probed), Some(full)) = (&probed, &full) {
            if !same(probed, full) {
                series.mismatches += 1;
            }
        }
        Ok(())
    }

    fn series(
        &mut self,
        iterations: usize,
        mut step: impl FnMut(&mut Self) -> Result<(), String>,
    ) -> Result<Series, String> {
        let mut series = Series::default();
        for _ in 0..iterations {
            step(self)?;
            self.measure(&mut series)?;
        }
        Ok(series)
    }

    fn edit(&mut self, count: usize) -> Result<(), String> {
        self.revision += 1;
        for file in self.edits.iter().take(count) {
            let mut content = self.originals[file].clone();
            content.extend_from_slice(format!("monitor-cost {}\n", self.revision).as_bytes());
            std::fs::write(self.work.join(file), content)
                .map_err(|error| format!("{file}: {error}"))?;
        }
        std::thread::sleep(SETTLE);
        Ok(())
    }

    fn restore(&mut self) -> Result<(), String> {
        for (file, content) in &self.originals {
            std::fs::write(self.work.join(file), content)
                .map_err(|error| format!("{file}: {error}"))?;
        }
        std::thread::sleep(SETTLE);
        self.probe_monitor()?;
        Ok(())
    }

    fn git_add(&mut self, iterations: usize) -> Result<Series, String> {
        let mut series = Series::default();
        let staged: Vec<String> = self.edits.iter().take(STAGED).cloned().collect();
        for _ in 0..iterations {
            self.edit(STAGED)?;
            self.probe_monitor()?;
            self.git_paths(&["add", "-f", "--"], &staged)?;
            std::thread::sleep(SETTLE);
            self.measure(&mut series)?;
            self.git_paths(&["reset", "-q", "--"], &staged)?;
            self.restore()?;
        }
        Ok(series)
    }

    fn git_commit(&mut self, iterations: usize) -> Result<Series, String> {
        let mut series = Series::default();
        let committed: Vec<String> = self.edits.iter().take(1).cloned().collect();
        for _ in 0..iterations {
            self.edit(1)?;
            self.git_paths(&["add", "-f", "--"], &committed)?;
            std::thread::sleep(SETTLE);
            self.probe_monitor()?;
            let mut args: Vec<&str> = IDENTITY.to_vec();
            args.extend(["commit", "-q", "--no-verify", "-m", "monitor-cost"]);
            git(&self.work, &args)?;
            std::thread::sleep(SETTLE);
            self.measure(&mut series)?;
        }
        Ok(series)
    }

    fn checkout(&mut self, iterations: usize) -> Result<Series, String> {
        let mut series = Series::default();
        let start = git_output(&self.work, &["rev-parse", "HEAD"])?;
        let edits = self.edits.clone();
        self.edit(EDITS)?;
        self.git_paths(&["add", "-f", "--"], &edits)?;
        let mut args: Vec<&str> = IDENTITY.to_vec();
        args.extend(["commit", "-q", "--no-verify", "-m", "monitor-cost branch"]);
        git(&self.work, &args)?;
        let other = git_output(&self.work, &["rev-parse", "HEAD"])?;
        git(&self.work, &["checkout", "-q", "--detach", start.trim()])?;
        std::thread::sleep(SETTLE);
        self.probe_monitor()?;
        for iteration in 0..iterations {
            let target = if iteration % 2 == 0 { &other } else { &start };
            git(&self.work, &["checkout", "-q", "--detach", target.trim()])?;
            std::thread::sleep(SETTLE);
            self.measure(&mut series)?;
        }
        git(&self.work, &["checkout", "-q", "--detach", start.trim()])?;
        std::thread::sleep(SETTLE);
        self.probe_monitor()?;
        Ok(series)
    }

    fn concurrent(&mut self, iterations: usize) -> Result<Series, String> {
        let mut series = Series::default();
        for _ in 0..iterations {
            self.edit(1)?;
            let this: &Self = self;
            let cpu = process::self_cpu();
            let started = Instant::now();
            let kinds = std::thread::scope(|scope| {
                let probes: Vec<_> = (0..CONSUMERS)
                    .map(|_| scope.spawn(|| this.probe_monitor().map(|(_, _, report)| report.kind)))
                    .collect();
                probes
                    .into_iter()
                    .map(|probe| probe.join().unwrap_or_else(|_| Err("panicked".into())))
                    .collect::<Result<Vec<_>, String>>()
            })?;
            series.monitor.push(Cost {
                wall: started.elapsed(),
                cpu: process::self_cpu().saturating_sub(cpu),
            });
            for kind in kinds {
                *series.kinds.entry(kind_name(&kind)).or_default() += 1;
            }
            let cpu = process::self_cpu();
            let started = Instant::now();
            std::thread::scope(|scope| {
                let probes: Vec<_> = (0..CONSUMERS)
                    .map(|_| scope.spawn(|| this.probe_full().map(|_| ())))
                    .collect();
                probes
                    .into_iter()
                    .map(|probe| probe.join().unwrap_or_else(|_| Err("panicked".into())))
                    .collect::<Result<Vec<()>, String>>()
            })?;
            series.full.push(Cost {
                wall: started.elapsed(),
                cpu: process::self_cpu().saturating_sub(cpu),
            });
            let started = Instant::now();
            let children = std::thread::scope(|scope| {
                let probes: Vec<_> = (0..CONSUMERS)
                    .map(|_| scope.spawn(|| this.probe_cli()))
                    .collect();
                probes
                    .into_iter()
                    .map(|probe| probe.join().unwrap_or_else(|_| Err("panicked".into())))
                    .collect::<Result<Vec<Cost>, String>>()
            })?;
            series.cli.push(Cost {
                wall: started.elapsed(),
                cpu: children.iter().map(|cost| cost.cpu).sum(),
            });
        }
        self.restore()?;
        Ok(series)
    }

    fn git_paths(&self, command: &[&str], paths: &[String]) -> Result<(), String> {
        let mut args: Vec<&str> = command.to_vec();
        args.extend(paths.iter().map(String::as_str));
        git(&self.work, &args)
    }
}

fn spread_files(work: &Path, count: usize) -> Result<Vec<String>, String> {
    let listing = git_output(work, &["ls-files", "-s"])?;
    let regular: Vec<&str> = listing
        .lines()
        .filter_map(|line| {
            let (meta, path) = line.split_once('\t')?;
            meta.starts_with("100644 ").then_some(path)
        })
        .filter(|path| {
            let name = path.rsplit('/').next().unwrap_or(path);
            !INPUT_NAMES.contains(&name)
        })
        .filter(|path| work.join(path).is_file())
        .collect();
    if regular.len() < count {
        return Err(format!(
            "{} holds {} regular tracked files, fewer than {count}",
            work.display(),
            regular.len()
        ));
    }
    let step = regular.len() / count;
    Ok(regular
        .iter()
        .step_by(step)
        .take(count)
        .map(|path| (*path).to_string())
        .collect())
}

fn same(left: &Snapshot, right: &Snapshot) -> bool {
    left.head == right.head
        && left.entries == right.entries
        && left.sides == right.sides
        && left.truncated == right.truncated
}

fn kind_name(kind: &ProbeKind) -> String {
    match kind {
        ProbeKind::Incremental => "incremental".to_string(),
        other => format!("{other:?}")
            .split(['(', ' '])
            .next()
            .unwrap_or("other")
            .to_lowercase(),
    }
}

fn git(cwd: &Path, args: &[&str]) -> Result<(), String> {
    git_output(cwd, args).map(drop)
}

fn git_output(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .env_remove(PRELOAD_ENV)
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|error| format!("git {}: {error}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn percentile(values: &mut [Duration], percent: usize) -> Duration {
    values.sort_unstable();
    let index = (values.len() * percent / 100).min(values.len().saturating_sub(1));
    values.get(index).copied().unwrap_or_default()
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn mebibytes(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn summary(costs: &[Cost]) -> String {
    if costs.is_empty() {
        return "n=0".to_string();
    }
    let mut walls: Vec<Duration> = costs.iter().map(|cost| cost.wall).collect();
    let mut cpus: Vec<Duration> = costs.iter().map(|cost| cost.cpu).collect();
    let total: Duration = cpus.iter().sum();
    let mean = total
        .checked_div(u32::try_from(costs.len()).unwrap_or(u32::MAX))
        .unwrap_or_default();
    format!(
        "n={} wall_p50_ms={:.2} wall_p95_ms={:.2} cpu_mean_ms={:.2} cpu_p50_ms={:.2} cpu_p95_ms={:.2}",
        costs.len(),
        millis(percentile(&mut walls, 50)),
        millis(percentile(&mut walls, 95)),
        millis(mean),
        millis(percentile(&mut cpus, 50)),
        millis(percentile(&mut cpus, 95)),
    )
}

fn report(scenario: &str, series: &Series) {
    let kinds: Vec<String> = series
        .kinds
        .iter()
        .map(|(kind, count)| format!("{kind}:{count}"))
        .collect();
    let rechecked_p50 = median(&series.rechecked);
    let hashed_p50 = median(&series.hashed);
    let stat_p50 = median(&series.stat_calls);
    println!(
        "scenario={scenario} engine=monitor {} kinds={} rechecked_p50={rechecked_p50} hashed_p50={hashed_p50} stat_calls_p50={stat_p50} mismatches={}",
        summary(&series.monitor),
        kinds.join(","),
        series.mismatches
    );
    println!("scenario={scenario} engine=full {}", summary(&series.full));
    println!("scenario={scenario} engine=cli {}", summary(&series.cli));
}

fn median(values: &[usize]) -> usize {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted.get(sorted.len() / 2).copied().unwrap_or(0)
}
