use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use paneflow_git::{snapshot, Options};

const CLI_ARGS: &[&str] = &[
    "--no-optional-locks",
    "status",
    "--porcelain=v1",
    "-z",
    "-uall",
    "--no-renames",
];

const DEFAULT_ITERATIONS: usize = 200;

const WARMUP_ITERATIONS: usize = 5;

const USAGE: &str = "usage: probe_cost <repository> <cli|gix1|gix2|gixall> [iterations]";

struct Sample {
    wall: Duration,
    cpu: Duration,
    child_peak_bytes: Option<u64>,
    entries: usize,
}

enum Mode {
    Cli,
    Gix(usize),
}

fn parse_mode(text: &str) -> Option<Mode> {
    match text {
        "cli" => Some(Mode::Cli),
        "gixall" => Some(Mode::Gix(
            std::thread::available_parallelism().map_or(1, usize::from),
        )),
        other => other
            .strip_prefix("gix")
            .and_then(|threads| threads.parse().ok())
            .filter(|threads| *threads > 0)
            .map(Mode::Gix),
    }
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some(repository), Some(mode_text)) = (args.next(), args.next()) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let Some(mode) = parse_mode(&mode_text) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let iterations = args
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_ITERATIONS);
    let repository = PathBuf::from(repository);
    let baseline_peak = process::self_peak_bytes();
    for _ in 0..WARMUP_ITERATIONS {
        if let Err(message) = probe(&repository, &mode) {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    }
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        match probe(&repository, &mode) {
            Ok(sample) => samples.push(sample),
            Err(message) => {
                eprintln!("{message}");
                return ExitCode::FAILURE;
            }
        }
    }
    report(
        &mode_text,
        &samples,
        baseline_peak,
        process::self_peak_bytes(),
    );
    ExitCode::SUCCESS
}

fn probe(repository: &Path, mode: &Mode) -> Result<Sample, String> {
    match mode {
        Mode::Cli => cli_probe(repository),
        Mode::Gix(threads) => gix_probe(repository, *threads),
    }
}

fn cli_probe(repository: &Path) -> Result<Sample, String> {
    let mut command = Command::new("git");
    command.args(CLI_ARGS).current_dir(repository);
    let started = Instant::now();
    let child = process::run(command).map_err(|error| format!("git status: {error}"))?;
    let wall = started.elapsed();
    if !child.success {
        return Err(format!(
            "git status failed: {}",
            String::from_utf8_lossy(&child.stderr)
        ));
    }
    Ok(Sample {
        wall,
        cpu: child.cpu,
        child_peak_bytes: Some(child.peak_bytes),
        entries: child
            .stdout
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
            .count(),
    })
}

fn gix_probe(repository: &Path, threads: usize) -> Result<Sample, String> {
    let options = Options {
        threads: paneflow_git::Threads::Fixed(threads),
        ..Options::default()
    };
    let cpu_before = process::self_cpu();
    let started = Instant::now();
    let snapshot = snapshot(repository, &options).map_err(|error| format!("gix: {error}"))?;
    let wall = started.elapsed();
    let cpu = process::self_cpu().saturating_sub(cpu_before);
    Ok(Sample {
        wall,
        cpu,
        child_peak_bytes: None,
        entries: snapshot.entries.len(),
    })
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

fn report(mode: &str, samples: &[Sample], baseline_peak: u64, final_peak: u64) {
    let mut walls: Vec<Duration> = samples.iter().map(|sample| sample.wall).collect();
    let mut cpus: Vec<Duration> = samples.iter().map(|sample| sample.cpu).collect();
    let entries = samples.last().map_or(0, |sample| sample.entries);
    let cpu_total: Duration = cpus.iter().sum();
    let cpu_mean = cpu_total
        .checked_div(u32::try_from(samples.len()).unwrap_or(u32::MAX))
        .unwrap_or_default();
    let mut line = format!(
        "mode={mode} iterations={} entries={entries} wall_p50_ms={:.2} wall_p95_ms={:.2} cpu_mean_ms={:.2} cpu_p50_ms={:.2} cpu_p95_ms={:.2}",
        samples.len(),
        millis(percentile(&mut walls, 50)),
        millis(percentile(&mut walls, 95)),
        millis(cpu_mean),
        millis(percentile(&mut cpus, 50)),
        millis(percentile(&mut cpus, 95)),
    );
    let mut child_peaks: Vec<u64> = samples
        .iter()
        .filter_map(|sample| sample.child_peak_bytes)
        .collect();
    if child_peaks.is_empty() {
        line.push_str(&format!(
            " process_peak_mib={:.1} baseline_peak_mib={:.1} growth_mib={:.1}",
            mebibytes(final_peak),
            mebibytes(baseline_peak),
            mebibytes(final_peak.saturating_sub(baseline_peak)),
        ));
    } else {
        child_peaks.sort_unstable();
        let median = child_peaks[child_peaks.len() / 2];
        let max = child_peaks.last().copied().unwrap_or_default();
        line.push_str(&format!(
            " child_peak_p50_mib={:.1} child_peak_max_mib={:.1}",
            mebibytes(median),
            mebibytes(max)
        ));
    }
    println!("{line}");
}

struct ChildRun {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    cpu: Duration,
    peak_bytes: u64,
}

#[cfg(unix)]
mod process {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    use super::ChildRun;

    #[cfg(target_os = "macos")]
    const MAXRSS_UNIT: u64 = 1;

    #[cfg(not(target_os = "macos"))]
    const MAXRSS_UNIT: u64 = 1024;

    fn timeval(value: libc::timeval) -> Duration {
        Duration::from_secs(value.tv_sec as u64) + Duration::from_micros(value.tv_usec as u64)
    }

    fn usage(who: libc::c_int) -> libc::rusage {
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(who, &mut usage) };
        usage
    }

    pub(super) fn self_cpu() -> Duration {
        let usage = usage(libc::RUSAGE_SELF);
        timeval(usage.ru_utime) + timeval(usage.ru_stime)
    }

    pub(super) fn self_peak_bytes() -> u64 {
        usage(libc::RUSAGE_SELF).ru_maxrss as u64 * MAXRSS_UNIT
    }

    pub(super) fn run(mut command: Command) -> std::io::Result<ChildRun> {
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        if let Some(mut pipe) = child.stdout.take() {
            pipe.read_to_end(&mut stdout)?;
        }
        if let Some(mut pipe) = child.stderr.take() {
            pipe.read_to_end(&mut stderr)?;
        }
        let mut status = 0;
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        let pid = child.id() as libc::pid_t;
        let waited = unsafe { libc::wait4(pid, &mut status, 0, &mut usage) };
        if waited != pid {
            return Err(std::io::Error::last_os_error());
        }
        Ok(ChildRun {
            success: libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
            stdout,
            stderr,
            cpu: timeval(usage.ru_utime) + timeval(usage.ru_stime),
            peak_bytes: usage.ru_maxrss as u64 * MAXRSS_UNIT,
        })
    }
}

#[cfg(windows)]
mod process {
    use std::io::Read;
    use std::os::windows::io::AsRawHandle;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{FILETIME, HANDLE};
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    use super::ChildRun;

    const FILETIME_TICK_NANOS: u64 = 100;

    fn ticks(time: FILETIME) -> u64 {
        (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
    }

    fn cpu(handle: HANDLE) -> Duration {
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut creation, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
        unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
        Duration::from_nanos((ticks(kernel) + ticks(user)) * FILETIME_TICK_NANOS)
    }

    fn peak_bytes(handle: HANDLE) -> u64 {
        let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        counters.cb = size;
        unsafe { GetProcessMemoryInfo(handle, &mut counters, size) };
        counters.PeakWorkingSetSize as u64
    }

    pub(super) fn self_cpu() -> Duration {
        cpu(unsafe { GetCurrentProcess() })
    }

    pub(super) fn self_peak_bytes() -> u64 {
        peak_bytes(unsafe { GetCurrentProcess() })
    }

    pub(super) fn run(mut command: Command) -> std::io::Result<ChildRun> {
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        if let Some(mut pipe) = child.stdout.take() {
            pipe.read_to_end(&mut stdout)?;
        }
        if let Some(mut pipe) = child.stderr.take() {
            pipe.read_to_end(&mut stderr)?;
        }
        let status = child.wait()?;
        let handle = child.as_raw_handle() as HANDLE;
        Ok(ChildRun {
            success: status.success(),
            stdout,
            stderr,
            cpu: cpu(handle),
            peak_bytes: peak_bytes(handle),
        })
    }
}
