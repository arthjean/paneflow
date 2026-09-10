use std::path::{Path, PathBuf};

use paneflow_browser_protocol::Availability;

const MANIFEST: &str = include_str!("../../../native/browser/manifest.toml");

pub const RUNTIME_SUBDIR: &str = "lib/paneflow/browser";
pub const HOST_SUBDIR: &str = "lib/paneflow/paneflow-browser-host.exe";
pub const STAMP: &str = "verified-manifest.sha256";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Environment,
    Installed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub host_binary: PathBuf,
    pub runtime_root: PathBuf,
    pub source: Source,
}

pub fn target_triple() -> String {
    format!("{}-pc-windows-msvc", std::env::consts::ARCH)
}

pub fn declared_availability() -> Availability {
    declared_availability_in(MANIFEST, &target_triple())
}

pub fn required_bytes() -> u64 {
    required_bytes_in(MANIFEST, &target_triple())
}

pub fn required_bytes_in(manifest: &str, target: &str) -> u64 {
    toml::from_str::<toml::Value>(manifest)
        .ok()
        .and_then(|value| {
            value
                .get("targets")?
                .get(target)?
                .get("unpacked_size")?
                .as_integer()
                .and_then(|size| u64::try_from(size).ok())
        })
        .unwrap_or(0)
}

pub fn declared_availability_in(manifest: &str, target: &str) -> Availability {
    let Ok(value) = toml::from_str::<toml::Value>(manifest) else {
        return Availability::Absent;
    };
    match value
        .get("targets")
        .and_then(|targets| targets.get(target))
        .and_then(|entry| entry.get("availability"))
        .and_then(toml::Value::as_str)
    {
        Some("development") => Availability::Development,
        Some("human_qualified") => Availability::HumanQualified,
        Some("agent_qualified") => Availability::AgentQualified,
        _ => Availability::Absent,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sandbox {
    Bootstrap,
}

impl Sandbox {
    pub fn label(self) -> &'static str {
        match self {
            Self::Bootstrap => "CEF bootstrap and renderer sandbox",
        }
    }
}

pub fn sandbox_mechanism(runtime_root: &Path, host_binary: &Path) -> Result<Sandbox, String> {
    if !runtime_root.join("Release/libcef.dll").is_file() {
        return Err("the Windows CEF runtime has no Release/libcef.dll".to_string());
    }
    if !host_binary
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("exe"))
    {
        return Err("the Windows browser host must be a bootstrap .exe".to_string());
    }
    verify_windows_pe(host_binary, "Windows browser bootstrap", false)?;
    let client = client_binary(host_binary);
    if !client.is_file() {
        return Err(format!(
            "the Windows browser host has no client DLL next to {}",
            host_binary.display()
        ));
    }
    verify_windows_pe(&client, "Windows browser client DLL", true)?;
    Ok(Sandbox::Bootstrap)
}

pub(crate) fn verify_windows_pe(
    path: &Path,
    label: &str,
    require_run_win_main: bool,
) -> Result<(), String> {
    let metadata =
        std::fs::metadata(path).map_err(|error| format!("the {label} is unreadable: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("the {label} is not a regular file"));
    }
    let bytes =
        std::fs::read(path).map_err(|error| format!("the {label} is unreadable: {error}"))?;
    inspect_windows_pe(&bytes, label, require_run_win_main)
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn rva_offset(bytes: &[u8], sections_offset: usize, sections: usize, rva: u32) -> Option<usize> {
    for index in 0..sections {
        let section = sections_offset.checked_add(index.checked_mul(40)?)?;
        let virtual_size = read_u32(bytes, section.checked_add(8)?)?;
        let virtual_address = read_u32(bytes, section.checked_add(12)?)?;
        let raw_size = read_u32(bytes, section.checked_add(16)?)?;
        let raw_offset = read_u32(bytes, section.checked_add(20)?)?;
        let span = virtual_size.max(raw_size);
        if rva >= virtual_address && rva - virtual_address < span {
            let offset = usize::try_from(raw_offset)
                .ok()?
                .checked_add(usize::try_from(rva - virtual_address).ok()?)?;
            if bytes.get(offset).is_some() {
                return Some(offset);
            }
        }
    }
    None
}

fn inspect_windows_pe(bytes: &[u8], label: &str, require_run_win_main: bool) -> Result<(), String> {
    let invalid = || format!("the {label} is not a valid x86_64 PE image");
    if bytes.get(..2) != Some(b"MZ") {
        return Err(format!("the {label} is not a PE image"));
    }
    let pe_offset =
        usize::try_from(read_u32(bytes, 0x3c).ok_or_else(invalid)?).map_err(|_| invalid())?;
    if bytes.get(pe_offset..pe_offset.checked_add(4).ok_or_else(invalid)?) != Some(b"PE\0\0") {
        return Err(invalid());
    }
    let coff = pe_offset.checked_add(4).ok_or_else(invalid)?;
    if read_u16(bytes, coff).ok_or_else(invalid)? != 0x8664 {
        return Err(invalid());
    }
    let sections =
        usize::from(read_u16(bytes, coff.checked_add(2).ok_or_else(invalid)?).ok_or_else(invalid)?);
    let optional_size = usize::from(
        read_u16(bytes, coff.checked_add(16).ok_or_else(invalid)?).ok_or_else(invalid)?,
    );
    let optional = coff.checked_add(20).ok_or_else(invalid)?;
    if read_u16(bytes, optional).ok_or_else(invalid)? != 0x20b || optional_size < 112 {
        return Err(invalid());
    }
    let directories =
        read_u32(bytes, optional.checked_add(108).ok_or_else(invalid)?).ok_or_else(invalid)?;
    let export_rva = if directories == 0 {
        0
    } else {
        read_u32(bytes, optional.checked_add(112).ok_or_else(invalid)?).ok_or_else(invalid)?
    };
    if !require_run_win_main {
        return Ok(());
    }
    if export_rva == 0 || directories < 1 {
        return Err(format!("the {label} does not export RunWinMain"));
    }
    let sections_offset = optional.checked_add(optional_size).ok_or_else(invalid)?;
    let export = rva_offset(bytes, sections_offset, sections, export_rva).ok_or_else(invalid)?;
    let functions =
        read_u32(bytes, export.checked_add(28).ok_or_else(invalid)?).ok_or_else(invalid)?;
    let names = read_u32(bytes, export.checked_add(32).ok_or_else(invalid)?).ok_or_else(invalid)?;
    let ordinals =
        read_u32(bytes, export.checked_add(36).ok_or_else(invalid)?).ok_or_else(invalid)?;
    let function_count =
        read_u32(bytes, export.checked_add(20).ok_or_else(invalid)?).ok_or_else(invalid)?;
    let name_count =
        read_u32(bytes, export.checked_add(24).ok_or_else(invalid)?).ok_or_else(invalid)?;
    let names_offset = rva_offset(bytes, sections_offset, sections, names).ok_or_else(invalid)?;
    let ordinals_offset =
        rva_offset(bytes, sections_offset, sections, ordinals).ok_or_else(invalid)?;
    let functions_offset =
        rva_offset(bytes, sections_offset, sections, functions).ok_or_else(invalid)?;
    for index in 0..usize::try_from(name_count).map_err(|_| invalid())? {
        let name_rva = read_u32(
            bytes,
            names_offset
                .checked_add(index.checked_mul(4).ok_or_else(invalid)?)
                .ok_or_else(invalid)?,
        )
        .ok_or_else(invalid)?;
        let name_offset =
            rva_offset(bytes, sections_offset, sections, name_rva).ok_or_else(invalid)?;
        let length = bytes
            .get(name_offset..)
            .and_then(|value| value.iter().position(|byte| *byte == 0))
            .ok_or_else(invalid)?;
        if bytes.get(name_offset..name_offset.checked_add(length).ok_or_else(invalid)?)
            == Some(b"RunWinMain")
        {
            let ordinal = read_u16(
                bytes,
                ordinals_offset
                    .checked_add(index.checked_mul(2).ok_or_else(invalid)?)
                    .ok_or_else(invalid)?,
            )
            .ok_or_else(invalid)?;
            if u32::from(ordinal) >= function_count {
                return Err(invalid());
            }
            let function_rva = read_u32(
                bytes,
                functions_offset
                    .checked_add(usize::from(ordinal).checked_mul(4).ok_or_else(invalid)?)
                    .ok_or_else(invalid)?,
            )
            .ok_or_else(invalid)?;
            if function_rva != 0
                && rva_offset(bytes, sections_offset, sections, function_rva).is_some()
            {
                return Ok(());
            }
            return Err(invalid());
        }
    }
    Err(format!("the {label} does not export RunWinMain"))
}

pub fn client_binary(host_binary: &Path) -> PathBuf {
    host_binary.with_extension("dll")
}

pub fn locate() -> Option<Layout> {
    from_environment(
        std::env::var_os(super::RUNTIME_ENV).map(PathBuf::from),
        std::env::var_os(super::HOST_ENV).map(PathBuf::from),
        std::env::current_exe().ok().as_deref(),
    )
    .or_else(|| from_prefix(std::env::current_exe().ok()?.as_path()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Readiness {
    Ready(Layout, Sandbox),
    Unusable(String),
    Absent,
}

pub fn detect() -> Readiness {
    match locate() {
        Some(layout) => classify(layout, &super::supervisor::manifest_digest()),
        None => Readiness::Absent,
    }
}

pub fn classify(layout: Layout, expected: &str) -> Readiness {
    match std::fs::read_to_string(layout.runtime_root.join(STAMP)) {
        Ok(stamp) if stamp.trim() == expected => {
            match sandbox_mechanism(&layout.runtime_root, &layout.host_binary) {
                Ok(sandbox) => Readiness::Ready(layout, sandbox),
                Err(reason) => Readiness::Unusable(reason),
            }
        }
        Ok(stamp) => Readiness::Unusable(format!(
            "the installed Windows browser runtime at {} was verified against manifest {}, but this build expects {}",
            layout.runtime_root.display(),
            stamp.trim(),
            expected
        )),
        Err(error) if layout.source == Source::Environment => Readiness::Unusable(format!(
            "the Windows browser runtime at {} has no verification stamp: {error}",
            layout.runtime_root.display()
        )),
        Err(error) => Readiness::Unusable(format!(
            "the installed Windows browser runtime at {} is unreadable: {error}",
            layout.runtime_root.display()
        )),
    }
}

fn sibling_host(current_exe: &Path) -> Option<PathBuf> {
    let directory = current_exe.parent()?;
    ["paneflow-browser-host.exe", "paneflow-browser-host.dll"]
        .into_iter()
        .map(|name| directory.join(name))
        .find(|path| path.is_file())
}

pub fn from_environment(
    runtime_root: Option<PathBuf>,
    host_binary: Option<PathBuf>,
    current_exe: Option<&Path>,
) -> Option<Layout> {
    let runtime_root = runtime_root?;
    let host_binary = host_binary.or_else(|| current_exe.and_then(sibling_host))?;
    Some(Layout {
        host_binary,
        runtime_root,
        source: Source::Environment,
    })
}

pub fn from_prefix(current_exe: &Path) -> Option<Layout> {
    let prefix = current_exe.parent()?.parent()?;
    let runtime_root = prefix.join(RUNTIME_SUBDIR);
    let host_binary = prefix.join(HOST_SUBDIR);
    if !runtime_root.join(STAMP).is_file() || !host_binary.is_file() {
        return None;
    }
    Some(Layout {
        host_binary,
        runtime_root,
        source: Source::Installed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "paneflow-browser-install-windows-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|moment| moment.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn install(prefix: &Path) -> PathBuf {
        std::fs::create_dir_all(prefix.join("bin")).unwrap();
        std::fs::create_dir_all(prefix.join(RUNTIME_SUBDIR).join("Release")).unwrap();
        std::fs::create_dir_all(prefix.join(HOST_SUBDIR).parent().unwrap()).unwrap();
        std::fs::write(prefix.join("bin/paneflow.exe"), b"app").unwrap();
        std::fs::write(prefix.join(HOST_SUBDIR), pe_image(false)).unwrap();
        std::fs::write(
            prefix.join(HOST_SUBDIR).with_extension("dll"),
            pe_image(true),
        )
        .unwrap();
        std::fs::write(
            prefix.join(RUNTIME_SUBDIR).join("Release/libcef.dll"),
            b"MZcef",
        )
        .unwrap();
        std::fs::write(prefix.join(RUNTIME_SUBDIR).join(STAMP), b"digest\n").unwrap();
        prefix.join("bin/paneflow.exe")
    }

    #[test]
    fn the_windows_manifest_exposes_development_without_native_qualification() {
        assert_eq!(declared_availability(), Availability::Development);
    }

    #[test]
    fn the_windows_manifest_declares_the_unpacked_payload_size() {
        assert!(required_bytes() > 128 * 1024 * 1024);
        assert_eq!(required_bytes_in("", &target_triple()), 0);
        assert_eq!(required_bytes_in(MANIFEST, "x86_64-unknown-none"), 0);
    }

    #[test]
    fn a_runtime_stamped_by_another_manifest_is_refused_instead_of_mixed() {
        let root = scratch("mixed");
        let exe = install(&root.join("usr"));
        let layout = from_prefix(&exe).unwrap();
        let Readiness::Unusable(reason) = classify(layout.clone(), "expected-digest") else {
            panic!("a stamp written by another manifest must keep the runtime unusable");
        };
        assert!(reason.contains("expected-digest"));
        assert!(matches!(
            classify(layout, "digest"),
            Readiness::Ready(_, Sandbox::Bootstrap)
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_installed_prefix_requires_the_bootstrap_executable_and_runtime_dll() {
        let root = scratch("prefix");
        let exe = install(&root.join("usr"));
        let layout = from_prefix(&exe).unwrap();
        assert_eq!(layout.source, Source::Installed);
        assert_eq!(layout.host_binary, root.join("usr").join(HOST_SUBDIR));
        assert!(matches!(
            sandbox_mechanism(&layout.runtime_root, &layout.host_binary),
            Ok(Sandbox::Bootstrap)
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_dll_cannot_be_promoted_to_the_windows_bootstrap_role() {
        let root = scratch("dll");
        std::fs::create_dir_all(root.join("Release")).unwrap();
        std::fs::write(root.join("Release/libcef.dll"), b"MZcef").unwrap();
        let error = sandbox_mechanism(&root, Path::new("host.dll")).unwrap_err();
        assert!(error.contains("bootstrap"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_unknown_client_dll_keeps_the_windows_host_unavailable() {
        let root = scratch("client");
        let exe = install(&root.join("usr"));
        std::fs::write(
            root.join("usr").join(HOST_SUBDIR).with_extension("dll"),
            pe_image(false),
        )
        .unwrap();
        let error = sandbox_mechanism(
            &root.join("usr").join(RUNTIME_SUBDIR),
            &root.join("usr").join(HOST_SUBDIR),
        )
        .unwrap_err();
        assert!(error.contains("RunWinMain"));
        assert!(exe.is_file());
        let _ = std::fs::remove_dir_all(root);
    }

    fn pe_image(run_win_main: bool) -> Vec<u8> {
        let mut bytes = vec![0; 0x900];
        bytes[0..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&0x80_u32.to_le_bytes());
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        bytes[0x84..0x86].copy_from_slice(&0x8664_u16.to_le_bytes());
        bytes[0x86..0x88].copy_from_slice(&1_u16.to_le_bytes());
        bytes[0x94..0x96].copy_from_slice(&0xf0_u16.to_le_bytes());
        bytes[0x98..0x9a].copy_from_slice(&0x20b_u16.to_le_bytes());
        bytes[0x104..0x108].copy_from_slice(&1_u32.to_le_bytes());
        if run_win_main {
            bytes[0x108..0x10c].copy_from_slice(&0x1100_u32.to_le_bytes());
            bytes[0x10c..0x110].copy_from_slice(&0x200_u32.to_le_bytes());
        }
        let section = 0x188;
        bytes[section..section + 5].copy_from_slice(b".text");
        bytes[section + 8..section + 12].copy_from_slice(&0x500_u32.to_le_bytes());
        bytes[section + 12..section + 16].copy_from_slice(&0x1000_u32.to_le_bytes());
        bytes[section + 16..section + 20].copy_from_slice(&0x500_u32.to_le_bytes());
        bytes[section + 20..section + 24].copy_from_slice(&0x400_u32.to_le_bytes());
        if run_win_main {
            let export = 0x500;
            bytes[export + 16..export + 20].copy_from_slice(&1_u32.to_le_bytes());
            bytes[export + 20..export + 24].copy_from_slice(&1_u32.to_le_bytes());
            bytes[export + 24..export + 28].copy_from_slice(&1_u32.to_le_bytes());
            bytes[export + 28..export + 32].copy_from_slice(&0x1200_u32.to_le_bytes());
            bytes[export + 32..export + 36].copy_from_slice(&0x1210_u32.to_le_bytes());
            bytes[export + 36..export + 40].copy_from_slice(&0x1220_u32.to_le_bytes());
            bytes[0x600..0x604].copy_from_slice(&0x1400_u32.to_le_bytes());
            bytes[0x610..0x614].copy_from_slice(&0x1300_u32.to_le_bytes());
            bytes[0x620..0x622].copy_from_slice(&0_u16.to_le_bytes());
            bytes[0x700..0x70a].copy_from_slice(b"RunWinMain");
            bytes[0x70a] = 0;
            bytes[0x800] = 0xc3;
        }
        bytes
    }
}
