use std::time::Duration;

pub(crate) const CORPUS_SEED: u64 = 0x5041_4e45_464c_4f57;
const CORPUS_FAMILIES: usize = 27;
const CORPUS_VARIANTS: usize = 5;
const CORPUS_SIZE: usize = CORPUS_FAMILIES * CORPUS_VARIANTS;

pub(crate) fn deterministic_streams() -> Vec<Vec<u8>> {
    let mut streams = Vec::with_capacity(CORPUS_SIZE);
    for index in 0..CORPUS_SIZE {
        let variant = index / CORPUS_FAMILIES;
        let family = index % CORPUS_FAMILIES;
        let bytes = match family {
            0 => format!("plain-ascii-{variant}\r\n").into_bytes(),
            1 => format!("unicode-{variant}: café Καλημέρα हिन्दी 🦀\r\n").into_bytes(),
            2 => format!("grapheme-{variant}: e\u{301} n\u{303} 👨‍👩‍👧‍👦\r\n").into_bytes(),
            3 => format!("wide-{variant}: 中文 日本語 한글\r\n").into_bytes(),
            4 => format!("\x1b[1;3;4;9mstyled-{variant}\x1b[0m\r\n").into_bytes(),
            5 => format!(
                "\x1b[38;2;{};{};{}mtruecolor-{variant}\x1b[0m",
                20 + variant,
                80 + variant,
                140 + variant
            )
            .into_bytes(),
            6 => format!(
                "origin\x1b[{};{}Hcursor-{variant}\x1b[2A\x1b[3C",
                2 + variant,
                3 + variant
            )
            .into_bytes(),
            7 => (format!("wrap-{variant}-") + &"x".repeat(180 + variant)).into_bytes(),
            8 => (format!("reflow-{variant}-") + &"0123456789".repeat(24)).into_bytes(),
            9 => format!("before\x1b[?1049halt-{variant}\x1b[?1049lafter").into_bytes(),
            10 => (0..40)
                .map(|line| format!("scroll-{variant}-{line}\r\n"))
                .collect::<String>()
                .into_bytes(),
            11 => format!("\x1b[?1h\x1b[?1000h\x1b[?1006hmode-{variant}").into_bytes(),
            12 => format!("\x1b]2;synthetic-title-{variant}\x07title-body").into_bytes(),
            13 => format!("query-{variant}\x1b[5n\x1b[6n\x1b[c\x1b[>c").into_bytes(),
            14 => format!("malformed-{variant}\x1b[999999999999999999999;?;mend").into_bytes(),
            15 => {
                format!("truncated-{variant}\x1b]8;;https://synthetic.invalid/unterminated")
                    .into_bytes()
            }
            16 => format!("erase-{variant}\x1b[2J\x1b[Hredrawn-{variant}").into_bytes(),
            17 => format!(
                "\x1b]8;id=synthetic-{variant};https://example.invalid/{variant}\x07link\x1b]8;;\x07"
            )
            .into_bytes(),
            18 => format!(
                "\x1b]133;A\x07prompt-{variant}\x1b]133;B\x07command\x1b]133;C\x07output\x1b]133;D;0\x07"
            )
            .into_bytes(),
            19 => format!("\x1b]52;c;c3ludGhldGljLWNsaXBib2FyZC0{variant}=\x07").into_bytes(),
            20 => format!(
                "\x1b[{};{}mansi16-{variant}\x1b[0m",
                30 + variant,
                40 + ((variant + 2) % 6)
            )
            .into_bytes(),
            21 => format!(
                "\x1b[38;5;{};48;5;{}mindexed256-{variant}\x1b[0m",
                16 + variant * 17,
                231 - variant * 11
            )
            .into_bytes(),
            22 => format!("\x1b[2;7mdim-inverse-{variant}\x1b[0m").into_bytes(),
            23 => format!("\x1b[{} qcursor-shape-{variant}", variant + 1).into_bytes(),
            24 => {
                let mut bytes = format!("invalid-utf8-{variant}:").into_bytes();
                bytes.extend_from_slice(&[0xf0, 0x28, 0x8c, 0x28, b'\r', b'\n']);
                bytes
            }
            25 => format!("tabs-{variant}:\talpha\t中\tomega\r\n").into_bytes(),
            26 => format!("selection-{variant}-target").into_bytes(),
            _ => unreachable!(),
        };
        streams.push(bytes);
    }
    streams
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
