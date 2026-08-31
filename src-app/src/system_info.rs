//! The report behind Help > System Info (issue #37).
//!
//! A bug report's environment section: version and install format, OS,
//! display server, CPU, GPU, renderer, terminal engine. It carries no project
//! path, no repository name and no environment dump. The single environment
//! variable it reads is `XDG_CURRENT_DESKTOP`, named explicitly because the
//! desktop environment explains a whole class of window, cursor and backdrop
//! bugs on Linux.
//!
//! Collection is split in two so the render thread never blocks:
//!
//! - [`SystemInfoProbe::capture`] runs on the render thread and takes only
//!   what needs `&Window` (the GPU adapter, already cached by the renderer)
//!   or a compile-time constant.
//! - [`SystemInfoProbe::resolve`] does the blocking probes (`/etc/os-release`,
//!   `/proc/cpuinfo`, the Windows registry, `sysctl`, Metal enumeration) and
//!   must be called from a background task.
//!
//! This module is pure collection and formatting. The modal that shows the
//! result, and the button that copies it, live in
//! [`crate::app::system_info_dialog`].
//!
//! The field labels match the `Environment` section of
//! `.github/ISSUE_TEMPLATE/bug_report.md` on purpose: the copied block is
//! meant to replace that section wholesale, with no re-editing by the
//! reporter.

use std::fmt::{self, Display};

use gpui::{GpuSpecs, SharedString, Window};

use crate::update::install_method::InstallMethod;

/// The renderer GPUI drives on this target. GPUI exposes the adapter
/// (`GpuSpecs`) but not the graphics API behind it, and the mapping is
/// one-per-platform: Vulkan through wgpu on Linux, Metal on macOS, and the
/// DirectX 11 renderer on Windows.
const RENDERER: &str = if cfg!(target_os = "macos") {
    "Metal"
} else if cfg!(target_os = "windows") {
    "DirectX 11"
} else {
    "Vulkan"
};

/// Placeholder for a probe that came back empty. Kept as one constant so a
/// reader of an issue can tell "we asked and the system did not say" apart
/// from a field we never collect.
const UNKNOWN: &str = "unknown";

/// The render-thread half of the collection. Holds the values that cannot be
/// read from a background thread, and nothing that blocks.
pub(crate) struct SystemInfoProbe {
    /// `None` on macOS, where GPUI's platform window returns no specs at all;
    /// [`SystemInfoProbe::resolve`] falls back to Metal enumeration there.
    gpu: Option<GpuSpecs>,
    install: &'static str,
}

/// A finished, formattable report. Plain data: every field is already a
/// string, so [`Display`] is pure and unit-testable without a window, a GPU
/// or a host to probe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SystemInfo {
    version: &'static str,
    target_triple: &'static str,
    /// Set only when the process runs under CPU emulation (Rosetta 2, WOW64):
    /// the native host architecture, which differs from the one in
    /// `target_triple`.
    emulated_on: Option<&'static str>,
    install: &'static str,
    os: String,
    /// `Some` on Linux only: compositor plus desktop environment.
    display_server: Option<String>,
    cpu: String,
    gpu: String,
    renderer: &'static str,
    terminal_engine: String,
}

impl SystemInfoProbe {
    /// Take the render-thread half of the report. Cheap: `Window::gpu_specs`
    /// reads adapter information the renderer already holds.
    pub(crate) fn capture(window: &Window, install: &InstallMethod) -> Self {
        Self {
            gpu: window.gpu_specs(),
            install: crate::app::self_update_flow::install_method_label(install),
        }
    }

    /// Finish the report. **Blocking**: reads `/etc/os-release`,
    /// `/proc/cpuinfo`, the Windows registry or `sysctl` depending on the
    /// target. Call from `cx.background_spawn`, never from the render thread.
    pub(crate) fn resolve(self) -> SystemInfo {
        let host_arch = crate::update::checker::host_arch();
        SystemInfo {
            version: env!("CARGO_PKG_VERSION"),
            target_triple: env!("PANEFLOW_TARGET_TRIPLE"),
            emulated_on: (host_arch != std::env::consts::ARCH).then_some(host_arch),
            install: self.install,
            os: os_description(),
            display_server: display_server_description(),
            cpu: cpu_description(),
            gpu: gpu_description(self.gpu.as_ref()),
            renderer: RENDERER,
            terminal_engine: terminal_engine_description(),
        }
    }
}

impl SystemInfo {
    /// The report as label / value pairs, in reading order.
    ///
    /// One source of truth for both renderings: the System Info modal lays
    /// these out as a two-column table, and [`Display`] turns the same pairs
    /// into the Markdown bullets the Copy button puts on the clipboard. What
    /// the user reads and what they paste therefore cannot drift.
    pub(crate) fn rows(&self) -> Vec<(&'static str, SharedString)> {
        let mut version = format!("{} ({}", self.version, self.target_triple);
        if let Some(host) = self.emulated_on {
            version.push_str(&format!(", emulated on {host}"));
        }
        version.push_str(&format!(", {})", self.install));

        let mut rows: Vec<(&'static str, SharedString)> =
            vec![("Paneflow", version.into()), ("OS", self.os.clone().into())];
        if let Some(display_server) = &self.display_server {
            rows.push(("Display server", display_server.clone().into()));
        }
        rows.push(("CPU", self.cpu.clone().into()));
        rows.push(("GPU", self.gpu.clone().into()));
        rows.push(("Renderer", self.renderer.into()));
        rows.push(("Terminal engine", self.terminal_engine.clone().into()));
        rows
    }
}

impl Display for SystemInfo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, (label, value)) in self.rows().into_iter().enumerate() {
            if index > 0 {
                writeln!(formatter)?;
            }
            write!(formatter, "- **{label}**: {value}")?;
        }
        Ok(())
    }
}

// ── GPU ──────────────────────────────────────────────────────────────────

/// Render `GpuSpecs` as `<device> - <driver> <driver info>`, with the
/// software-rasterizer flag spelled out. That flag is the highest-signal bit
/// in the whole report: a user on llvmpipe complaining about frame rate is
/// diagnosed by this line alone.
fn format_gpu_specs(specs: &GpuSpecs) -> String {
    let device = specs.device_name.trim();
    let mut out = if device.is_empty() {
        UNKNOWN.to_string()
    } else {
        device.to_string()
    };

    let driver = [specs.driver_name.trim(), specs.driver_info.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if !driver.is_empty() {
        out.push_str(" - ");
        out.push_str(&driver);
    }

    if specs.is_software_emulated {
        out.push_str(" (software emulated)");
    }
    out
}

#[cfg(not(target_os = "macos"))]
fn gpu_description(specs: Option<&GpuSpecs>) -> String {
    specs.map_or_else(|| UNKNOWN.to_string(), format_gpu_specs)
}

/// macOS never yields `GpuSpecs` (GPUI's AppKit window returns `None`), so the
/// adapter is read straight from Metal.
#[cfg(target_os = "macos")]
fn gpu_description(specs: Option<&GpuSpecs>) -> String {
    if let Some(specs) = specs {
        return format_gpu_specs(specs);
    }
    metal_device_names().unwrap_or_else(|| UNKNOWN.to_string())
}

/// Every Metal device in the machine, in Metal's own order.
///
/// `MTLCopyAllDevices` is used rather than `MTLCreateSystemDefaultDevice`
/// deliberately: on a graphics-switching Mac the latter forces the system onto
/// the high-power GPU (Apple documents this on the function itself), which
/// would make "copy my system info" cost the user battery life. `MTLCopyAllDevices`
/// has no such effect and reports both GPUs of a dual-GPU Intel MacBook Pro
/// instead of just one.
///
/// Returns `None` when the machine reports no Metal device at all, which the
/// caller renders as `unknown` rather than an empty field.
#[cfg(target_os = "macos")]
fn metal_device_names() -> Option<String> {
    use objc2_metal::{MTLCopyAllDevices, MTLDevice};

    let devices = MTLCopyAllDevices();
    // `NSArray::iter` sits behind objc2-foundation's `NSEnumerator` feature,
    // which nothing in our dependency set is required to turn on. Indexing
    // needs only `NSArray` itself, which `objc2-metal/MTLDevice` already
    // enables.
    let names: Vec<String> = (0..devices.count())
        .map(|index| devices.objectAtIndex(index).name().to_string())
        .filter(|name| !name.trim().is_empty())
        .collect();
    (!names.is_empty()).then(|| names.join(", "))
}

// ── OS name and version ──────────────────────────────────────────────────

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn os_description() -> String {
    // Same lookup order as systemd's own: /etc is the administrator's copy,
    // /usr/lib the vendor's, /var/run the transient one some images write.
    for path in [
        "/etc/os-release",
        "/usr/lib/os-release",
        "/var/run/os-release",
    ] {
        if let Ok(content) = std::fs::read_to_string(path)
            && let Some(name) = parse_os_release(&content)
        {
            return name;
        }
    }
    UNKNOWN.to_string()
}

/// Pull a human-readable distribution name out of an `os-release` file.
///
/// `PRETTY_NAME` is the field the spec reserves for exactly this ("Fedora
/// Linux 44 (Workstation Edition)"). Falls back to `NAME` plus `VERSION_ID`
/// for the minority of images that ship no pretty name.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn parse_os_release(content: &str) -> Option<String> {
    fn field<'a>(content: &'a str, key: &str) -> Option<&'a str> {
        content.lines().find_map(|line| {
            let value = line.trim().strip_prefix(key)?.strip_prefix('=')?;
            // Values are shell-quoted when they contain spaces.
            let value = value.trim().trim_matches('"').trim_matches('\'').trim();
            (!value.is_empty()).then_some(value)
        })
    }

    if let Some(pretty) = field(content, "PRETTY_NAME") {
        return Some(pretty.to_string());
    }
    let name = field(content, "NAME")?;
    Some(match field(content, "VERSION_ID") {
        Some(version) => format!("{name} {version}"),
        None => name.to_string(),
    })
}

#[cfg(target_os = "macos")]
fn os_description() -> String {
    let version = sysctl_string(c"kern.osproductversion");
    let build = sysctl_string(c"kern.osversion");
    match (version, build) {
        (Some(version), Some(build)) => format!("macOS {version} (build {build})"),
        (Some(version), None) => format!("macOS {version}"),
        // `hw.model` ("MacBookPro18,3") is not a version, but it still tells a
        // triager which machine class the report came from.
        (None, _) => match sysctl_string(c"hw.model") {
            Some(model) => format!("macOS ({UNKNOWN} version, {model})"),
            None => format!("macOS ({UNKNOWN} version)"),
        },
    }
}

/// Read a string-valued `sysctl` by name. Two calls: one to size the buffer,
/// one to fill it. Returns `None` for a missing key or a value that is not
/// valid UTF-8.
#[cfg(target_os = "macos")]
fn sysctl_string(name: &std::ffi::CStr) -> Option<String> {
    let mut size: usize = 0;
    // SAFETY: standard `sysctlbyname` FFI. `name` is a valid NUL-terminated C
    // string, the output pointer is null so the call only reports the size it
    // would write into `size`, and the new-value pointer is null (read-only
    // query).
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 || size == 0 {
        return None;
    }

    let mut buffer = vec![0u8; size];
    // SAFETY: same call, now with a buffer of exactly the size the kernel just
    // asked for; `size` is passed by pointer and updated with the bytes
    // actually written.
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            buffer.as_mut_ptr().cast::<libc::c_void>(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return None;
    }

    buffer.truncate(size);
    let value = String::from_utf8(buffer).ok()?;
    let value = value.trim_end_matches('\0').trim();
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(target_os = "windows")]
fn os_description() -> String {
    let Ok(key) =
        windows_registry::LOCAL_MACHINE.open(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
    else {
        return "Windows".to_string();
    };

    let build: u32 = key
        .get_string("CurrentBuildNumber")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);

    // `ProductName` still reads "Windows 10 ..." on Windows 11: Microsoft never
    // rewrote the value, and the build number is the only reliable
    // discriminator (11 starts at 22000). Rewriting the edition keeps the
    // suffix ("Pro", "Enterprise") intact.
    let product = key
        .get_string("ProductName")
        .unwrap_or_else(|_| "Windows".to_string());
    let product = if build >= 22000 {
        product.replacen("Windows 10", "Windows 11", 1)
    } else {
        product
    };

    let mut out = product;
    // "23H2". Absent on LTSC and on builds older than 20H2.
    if let Ok(display_version) = key.get_string("DisplayVersion") {
        let display_version = display_version.trim();
        if !display_version.is_empty() {
            out.push(' ');
            out.push_str(display_version);
        }
    }
    if build > 0 {
        // UBR is the patch level ("22631.4460"), absent on very old builds.
        match key.get_u32("UBR") {
            Ok(ubr) => out.push_str(&format!(" (build {build}.{ubr})")),
            Err(_) => out.push_str(&format!(" (build {build})")),
        }
    }
    out
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "macos",
    target_os = "windows"
)))]
fn os_description() -> String {
    std::env::consts::OS.to_string()
}

// ── Display server ───────────────────────────────────────────────────────

/// Compositor plus desktop environment, e.g. `Wayland (GNOME)`.
///
/// `None` off Linux, where the windowing system is implied by the OS. The
/// desktop environment is the one environment variable this report reads:
/// GNOME/Wayland and KDE/X11 behave differently enough around window
/// decorations, cursors and backdrops that a Linux UI bug is hard to triage
/// without it.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn display_server_description() -> Option<String> {
    let compositor = gpui::guess_compositor();
    Some(match std::env::var("XDG_CURRENT_DESKTOP") {
        Ok(desktop) if !desktop.trim().is_empty() => {
            format!("{compositor} ({})", desktop.trim())
        }
        _ => compositor.to_string(),
    })
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
fn display_server_description() -> Option<String> {
    None
}

// ── CPU ──────────────────────────────────────────────────────────────────

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn cpu_description() -> String {
    if let Ok(content) = std::fs::read_to_string("/proc/cpuinfo")
        && let Some(model) = parse_cpuinfo_model(&content)
    {
        return model;
    }
    // aarch64 kernels publish no `model name` in /proc/cpuinfo. Single-board
    // and Apple-silicon-under-Linux machines name themselves in the device
    // tree instead ("Raspberry Pi 5 Model B Rev 1.0").
    if let Ok(model) = std::fs::read_to_string("/proc/device-tree/model") {
        let model = model.trim_end_matches('\0').trim();
        if !model.is_empty() {
            return model.to_string();
        }
    }
    UNKNOWN.to_string()
}

/// First `model name` (x86) or `Model` (some ARM kernels) line of
/// `/proc/cpuinfo`. Every core repeats the same value, so the first one is the
/// answer.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn parse_cpuinfo_model(content: &str) -> Option<String> {
    let field = |wanted: &str, case_sensitive: bool| -> Option<String> {
        content.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            let key = key.trim();
            let matched = if case_sensitive {
                key == wanted
            } else {
                key.eq_ignore_ascii_case(wanted)
            };
            if !matched {
                return None;
            }
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_string())
        })
    };

    field("model name", false).or_else(||
        // Some aarch64 kernels publish no `model name` and put the board name
        // under `Model` instead. Matched case-SENSITIVELY on purpose: x86
        // /proc/cpuinfo also carries a lowercase `model` field holding the
        // numeric CPU model id ("97"), printed *before* `model name`, and a
        // case-insensitive match reports that number as the CPU.
        field("Model", true))
}

#[cfg(target_os = "macos")]
fn cpu_description() -> String {
    // Apple Silicon answers "Apple M2 Pro" here, Intel Macs their marketing
    // CPU string. `hw.model` is the machine identifier, a usable last resort.
    sysctl_string(c"machdep.cpu.brand_string")
        .or_else(|| sysctl_string(c"hw.model"))
        .unwrap_or_else(|| UNKNOWN.to_string())
}

#[cfg(target_os = "windows")]
fn cpu_description() -> String {
    // The same value the Windows `systeminfo` and `wmic cpu get name` report,
    // written by the firmware at boot. HKLM is world-readable, so no elevation
    // is involved.
    windows_registry::LOCAL_MACHINE
        .open(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0")
        .and_then(|key| key.get_string("ProcessorNameString"))
        .map(|name| name.trim().to_string())
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| UNKNOWN.to_string())
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "macos",
    target_os = "windows"
)))]
fn cpu_description() -> String {
    UNKNOWN.to_string()
}

// ── Terminal engine ──────────────────────────────────────────────────────

/// libghostty's version plus the ABI version we link against. A terminal bug
/// that only reproduces on one release is usually an engine-version story, and
/// the pinned archive is invisible from the outside otherwise.
fn terminal_engine_description() -> String {
    let identity = paneflow_terminal_ghostty::build_identity();
    format!(
        "libghostty {} (API {})",
        paneflow_terminal_ghostty::GHOSTTY_APP_VERSION,
        identity.api_version
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SystemInfo {
        SystemInfo {
            version: "0.5.12",
            target_triple: "x86_64-unknown-linux-gnu",
            emulated_on: None,
            install: "targz",
            os: "Fedora Linux 44 (Workstation Edition)".to_string(),
            display_server: Some("Wayland (GNOME)".to_string()),
            cpu: "AMD Ryzen 9 7950X 16-Core Processor".to_string(),
            gpu: "AMD Radeon RX 7900 XTX - radv Mesa 25.1.2".to_string(),
            renderer: "Vulkan",
            terminal_engine: "libghostty 1.2.0 (API 3)".to_string(),
        }
    }

    #[test]
    fn report_is_a_paste_ready_markdown_block() {
        assert_eq!(
            sample().to_string(),
            "- **Paneflow**: 0.5.12 (x86_64-unknown-linux-gnu, targz)\n\
             - **OS**: Fedora Linux 44 (Workstation Edition)\n\
             - **Display server**: Wayland (GNOME)\n\
             - **CPU**: AMD Ryzen 9 7950X 16-Core Processor\n\
             - **GPU**: AMD Radeon RX 7900 XTX - radv Mesa 25.1.2\n\
             - **Renderer**: Vulkan\n\
             - **Terminal engine**: libghostty 1.2.0 (API 3)"
        );
    }

    /// The modal renders `rows()` and the Copy button renders `Display`. This
    /// pins them to the same content, so a field added to one cannot go
    /// missing from the other.
    #[test]
    fn what_the_modal_shows_is_what_the_copy_button_writes() {
        let info = sample();
        let rendered: Vec<String> = info.to_string().lines().map(str::to_string).collect();
        let rows = info.rows();
        assert_eq!(rendered.len(), rows.len());
        for (line, (label, value)) in rendered.iter().zip(rows) {
            assert_eq!(line, &format!("- **{label}**: {value}"));
        }
    }

    #[test]
    fn report_has_no_trailing_newline_so_it_pastes_inside_a_template() {
        assert!(!sample().to_string().ends_with('\n'));
    }

    #[test]
    fn display_server_line_is_dropped_off_linux() {
        let mut info = sample();
        info.display_server = None;
        let rendered = info.to_string();
        assert!(!rendered.contains("Display server"));
        // The lines around the omitted one still follow each other.
        assert!(rendered.contains(
            "- **OS**: Fedora Linux 44 (Workstation Edition)\n- **CPU**: AMD Ryzen 9 7950X"
        ));
    }

    #[test]
    fn emulation_is_called_out_next_to_the_target_triple() {
        let mut info = sample();
        info.target_triple = "x86_64-apple-darwin";
        info.emulated_on = Some("aarch64");
        assert!(
            info.to_string()
                .starts_with("- **Paneflow**: 0.5.12 (x86_64-apple-darwin, emulated on aarch64, ")
        );
    }

    #[test]
    fn gpu_specs_render_device_then_driver() {
        let specs = GpuSpecs {
            is_software_emulated: false,
            device_name: "AMD Radeon RX 7900 XTX (RADV NAVI31)".to_string(),
            driver_name: "radv".to_string(),
            driver_info: "Mesa 25.1.2".to_string(),
        };
        assert_eq!(
            format_gpu_specs(&specs),
            "AMD Radeon RX 7900 XTX (RADV NAVI31) - radv Mesa 25.1.2"
        );
    }

    #[test]
    fn software_rasterizers_are_named_as_such() {
        let specs = GpuSpecs {
            is_software_emulated: true,
            device_name: "llvmpipe (LLVM 20.1.8, 256 bits)".to_string(),
            driver_name: "llvmpipe".to_string(),
            driver_info: "Mesa 25.1.2".to_string(),
        };
        assert!(format_gpu_specs(&specs).ends_with("(software emulated)"));
    }

    #[test]
    fn gpu_specs_with_no_driver_strings_render_the_device_alone() {
        let specs = GpuSpecs {
            is_software_emulated: false,
            device_name: "NVIDIA GeForce RTX 4080".to_string(),
            driver_name: String::new(),
            driver_info: "  ".to_string(),
        };
        assert_eq!(format_gpu_specs(&specs), "NVIDIA GeForce RTX 4080");
    }

    #[test]
    fn an_empty_device_name_reads_as_unknown_not_as_an_empty_field() {
        let specs = GpuSpecs {
            is_software_emulated: false,
            device_name: "   ".to_string(),
            driver_name: "i915".to_string(),
            driver_info: String::new(),
        };
        assert_eq!(format_gpu_specs(&specs), "unknown - i915");
    }

    /// The only test that runs every probe for real, against the host the
    /// suite runs on. It cannot assert the values (they differ per machine),
    /// but it does assert the shape: no probe may return an empty field, and
    /// the block must stay one labelled bullet per line, which is what makes
    /// it paste cleanly into the issue templates.
    ///
    /// Run with `--nocapture` to eyeball the real report after adding a field.
    #[test]
    fn every_probe_yields_a_labelled_line_on_the_host() {
        let report = SystemInfoProbe {
            gpu: None,
            install: "tests",
        }
        .resolve()
        .to_string();
        println!("{report}");

        let expected_lines = if cfg!(any(target_os = "linux", target_os = "freebsd")) {
            7 // the display-server line only exists here
        } else {
            6
        };
        assert_eq!(report.lines().count(), expected_lines, "{report}");

        for line in report.lines() {
            let (label, value) = line
                .strip_prefix("- **")
                .and_then(|rest| rest.split_once("**: "))
                .expect("every line is a `- **Label**: value` bullet");
            assert!(!label.is_empty(), "unlabelled line: {line}");
            assert!(!value.trim().is_empty(), "empty value for {label}");
        }
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    fn os_release_prefers_the_pretty_name() {
        let content = "NAME=\"Fedora Linux\"\n\
                       VERSION_ID=44\n\
                       PRETTY_NAME=\"Fedora Linux 44 (Workstation Edition)\"\n";
        assert_eq!(
            parse_os_release(content).as_deref(),
            Some("Fedora Linux 44 (Workstation Edition)")
        );
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    fn os_release_falls_back_to_name_and_version_id() {
        let content = "ID=alpine\nNAME=\"Alpine Linux\"\nVERSION_ID=3.21.0\n";
        assert_eq!(
            parse_os_release(content).as_deref(),
            Some("Alpine Linux 3.21.0")
        );
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    fn os_release_without_a_version_still_yields_the_name() {
        assert_eq!(
            parse_os_release("NAME=\"Some Linux\"\n").as_deref(),
            Some("Some Linux")
        );
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    fn os_release_does_not_match_a_key_by_suffix() {
        // `VERSION_ID` must not be read as the `ID` field, and
        // `IMAGE_VERSION` must not be read as `VERSION`.
        let content = "VERSION_ID=44\nIMAGE_VERSION=1.2\n";
        assert_eq!(parse_os_release(content), None);
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    fn os_release_that_names_nothing_yields_none() {
        assert_eq!(parse_os_release("ID=weird\nPRETTY_NAME=\"\"\n"), None);
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    fn cpuinfo_returns_the_first_model_name() {
        let content = "processor\t: 0\n\
                       model name\t: AMD Ryzen 9 7950X 16-Core Processor\n\
                       \n\
                       processor\t: 1\n\
                       model name\t: AMD Ryzen 9 7950X 16-Core Processor\n";
        assert_eq!(
            parse_cpuinfo_model(content).as_deref(),
            Some("AMD Ryzen 9 7950X 16-Core Processor")
        );
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    fn cpuinfo_ignores_the_numeric_x86_model_field() {
        // The real shape on x86: a lowercase `model` holding the numeric model
        // id, printed before `model name`. Reading it case-insensitively as
        // the ARM `Model` key reports the CPU as "97".
        let content = "processor\t: 0\n\
                       vendor_id\t: AuthenticAMD\n\
                       cpu family\t: 25\n\
                       model\t\t: 97\n\
                       model name\t: AMD Ryzen 9 7950X 16-Core Processor\n";
        assert_eq!(
            parse_cpuinfo_model(content).as_deref(),
            Some("AMD Ryzen 9 7950X 16-Core Processor")
        );
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    fn cpuinfo_accepts_the_arm_model_key() {
        let content = "processor\t: 0\n\
                       BogoMIPS\t: 108.00\n\
                       Model\t\t: Raspberry Pi 5 Model B Rev 1.0\n";
        assert_eq!(
            parse_cpuinfo_model(content).as_deref(),
            Some("Raspberry Pi 5 Model B Rev 1.0")
        );
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    fn cpuinfo_without_a_model_yields_none() {
        // The shape an aarch64 kernel actually prints.
        let content = "processor\t: 0\n\
                       CPU implementer\t: 0x41\n\
                       CPU part\t: 0xd0c\n";
        assert_eq!(parse_cpuinfo_model(content), None);
    }
}
