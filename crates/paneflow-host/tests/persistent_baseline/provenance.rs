use super::*;

pub(super) fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub(super) fn executable_identity(path: &Path) -> Value {
    let bytes = std::fs::read(path).unwrap();
    json!({
        "path": path.display().to_string(),
        "bytes": bytes.len(),
        "fnv1a64": format!("{:016x}", fnv1a64(&bytes)),
    })
}

pub(super) fn diff_fingerprint() -> Value {
    diff_fingerprint_excluding(None)
}

pub(super) fn diff_fingerprint_excluding(own_output: Option<&Path>) -> Value {
    let Some(root) = git(&["rev-parse", "--show-toplevel"]) else {
        return json!({"dirty": null, "note": "repository root unavailable"});
    };
    let diff = Command::new("git")
        .args(["diff", "HEAD", "--binary"])
        .current_dir(&root)
        .output();
    let untracked = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard", "-z"])
        .current_dir(&root)
        .output();
    let (Ok(diff), Ok(untracked)) = (diff, untracked) else {
        return json!({"dirty": null, "note": "git inventory unavailable"});
    };
    if !diff.status.success() || !untracked.status.success() {
        return json!({"dirty": null, "note": "git inventory failed"});
    }
    let mut bytes = diff.stdout;
    let mut paths = Vec::new();
    for name in untracked
        .stdout
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        let path = String::from_utf8_lossy(name).into_owned();
        if own_output.and_then(Path::file_name) == Path::new(&path).file_name() {
            continue;
        }
        let Ok(contents) = std::fs::read(Path::new(&root).join(&path)) else {
            return json!({"dirty": true, "note": "untracked content unreadable", "path": path});
        };
        bytes.extend_from_slice(name);
        bytes.push(0);
        bytes.extend_from_slice(&(contents.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&contents);
        paths.push(path);
    }
    json!({
        "dirty": !bytes.is_empty(),
        "fnv1a64": format!("{:016x}", fnv1a64(&bytes)),
        "bytes": bytes.len(),
        "untracked_paths": paths,
    })
}

pub(super) fn toolchain() -> Value {
    let rustc = Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string());
    json!({
        "rustc": rustc,
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
    })
}

pub(super) fn machine() -> Value {
    json!({
        "os": std::env::consts::OS,
        "os_build": os_build(),
        "arch": std::env::consts::ARCH,
        "cpu_model": cpu_model(),
        "logical_cpus": std::thread::available_parallelism().map(usize::from).ok(),
        "total_ram_bytes": total_ram_bytes(),
        "hostname_hash": std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .ok()
            .map(|name| format!("{:016x}", fnv1a64(name.as_bytes()))),
    })
}

fn os_build() -> Option<String> {
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new("cmd");
        command
            .args(["/d", "/c", "ver"])
            .creation_flags(0x0800_0000);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = Command::new("uname");
        command.arg("-a");
        command
    };
    command
        .output()
        .ok()
        .filter(|result| result.status.success())
        .map(|result| String::from_utf8_lossy(&result.stdout).trim().to_string())
}

#[cfg(target_os = "linux")]
fn cpu_model() -> Option<String> {
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()?
        .lines()
        .find(|line| line.starts_with("model name"))
        .and_then(|line| line.split(':').nth(1))
        .map(|model| model.trim().to_string())
}

#[cfg(windows)]
fn cpu_model() -> Option<String> {
    std::env::var("PROCESSOR_IDENTIFIER").ok()
}

#[cfg(target_os = "macos")]
fn cpu_model() -> Option<String> {
    let output = Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
fn cpu_model() -> Option<String> {
    None
}

#[cfg(target_os = "linux")]
fn total_ram_bytes() -> Option<u64> {
    std::fs::read_to_string("/proc/meminfo")
        .ok()?
        .lines()
        .find(|line| line.starts_with("MemTotal:"))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|kib| kib.parse::<u64>().ok())
        .map(|kib| kib * 1024)
}

#[cfg(windows)]
fn total_ram_bytes() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    (unsafe { GlobalMemoryStatusEx(&mut status) } != 0).then_some(status.ullTotalPhys)
}

#[cfg(target_os = "macos")]
fn total_ram_bytes() -> Option<u64> {
    let output = Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
fn total_ram_bytes() -> Option<u64> {
    None
}
