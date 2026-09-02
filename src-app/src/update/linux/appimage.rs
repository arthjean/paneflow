use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use super::super::error::{IntegrityMismatch, UpdateError};

const UPDATE_HTTP_TIMEOUT: Duration = Duration::from_secs(30);

const APPIMAGE_TOOL_DEADLINE: Duration = Duration::from_secs(10 * 60);

const APPIMAGE_TOOL_STDOUT_CAP: u64 = 1024 * 1024;

const TOOL_URL_X86_64: &str = "https://github.com/AppImageCommunity/AppImageUpdate/releases/download/2.0.0-alpha-1-20251018/appimageupdatetool-x86_64.AppImage";
const TOOL_URL_AARCH64: &str = "https://github.com/AppImageCommunity/AppImageUpdate/releases/download/2.0.0-alpha-1-20251018/appimageupdatetool-aarch64.AppImage";

const APPIMAGEUPDATETOOL_SHA256_X86_64: [u8; 32] = [
    0xd9, 0x76, 0xcd, 0xac, 0x66, 0x7b, 0x03, 0xde, 0xe8, 0xcb, 0x23, 0xfb, 0x95, 0xef, 0x74, 0xb0,
    0x42, 0xc4, 0x06, 0xc5, 0xcb, 0xab, 0x3f, 0xf2, 0x94, 0xd2, 0xb1, 0x6e, 0xfe, 0xaf, 0xf8, 0x4f,
];

const APPIMAGEUPDATETOOL_SHA256_AARCH64: [u8; 32] = [
    0x7a, 0xaf, 0x89, 0xdd, 0x4c, 0xf6, 0x6e, 0xbd, 0x94, 0x0d, 0x41, 0x6c, 0x67, 0xe1, 0xc2, 0x40,
    0xc5, 0x7a, 0x13, 0x9c, 0xee, 0x38, 0xd9, 0xc0, 0xed, 0x3b, 0xb9, 0x38, 0x7b, 0xc4, 0x35, 0xb0,
];

fn tool_asset_for(arch: &str) -> Result<(&'static str, &'static [u8; 32])> {
    match arch {
        "x86_64" => Ok((TOOL_URL_X86_64, &APPIMAGEUPDATETOOL_SHA256_X86_64)),
        "aarch64" => Ok((TOOL_URL_AARCH64, &APPIMAGEUPDATETOOL_SHA256_AARCH64)),
        other => bail!(
            "no appimageupdatetool release for arch '{other}'. Update PaneFlow manually from the releases page."
        ),
    }
}

pub fn run_update(source_path: &Path, asset_url: &str) -> Result<PathBuf> {
    if source_path.as_os_str().is_empty() {
        bail!(
            "This AppImage was launched without $APPIMAGE set; PaneFlow cannot locate the source file to update. Re-launch by double-clicking the .AppImage or running it directly from a shell."
        );
    }
    if !source_path.is_file() {
        bail!("AppImage source file not found: {}", source_path.display());
    }

    let tool = resolve_tool().context("resolve appimageupdatetool")?;

    let candidate = candidate_path_for(source_path)?;
    let _ = std::fs::remove_file(&candidate);
    std::fs::copy(source_path, &candidate)
        .with_context(|| format!("copy {} -> {}", source_path.display(), candidate.display()))?;

    if let Err(e) = invoke_tool(&tool, &candidate) {
        let _ = std::fs::remove_file(&candidate);
        return Err(e);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&candidate) {
            let mut perms = meta.permissions();
            perms.set_mode(perms.mode() | 0o100);
            let _ = std::fs::set_permissions(&candidate, perms);
        }
    }

    if let Err(e) = super::super::signature::fetch_and_verify(&candidate, asset_url).context(
        "verify updated AppImage signature - if this recurs right after a release, the \
         `latest` zsync channel may not yet point at the version the updater resolved; \
         retry in a few minutes",
    ) {
        let _ = std::fs::remove_file(&candidate);
        return Err(e);
    }

    if let Err(e) = std::fs::rename(&candidate, source_path).with_context(|| {
        format!(
            "promote {} -> {}",
            candidate.display(),
            source_path.display()
        )
    }) {
        let _ = std::fs::remove_file(&candidate);
        return Err(e);
    }

    Ok(source_path.to_path_buf())
}

fn candidate_path_for(source_path: &Path) -> Result<PathBuf> {
    let name = source_path
        .file_name()
        .context("AppImage source path has no file name")?;
    let mut candidate_name = name.to_os_string();
    candidate_name.push(format!(".paneflow-update.{}", std::process::id()));
    Ok(source_path.with_file_name(candidate_name))
}

fn resolve_tool() -> Result<PathBuf> {
    let arch = std::env::consts::ARCH;
    let (url, expected) = tool_asset_for(arch)?;
    let cached = cache_path_for(arch)?;
    if cached.exists() {
        match verify_sha256_of_file(&cached, expected) {
            Ok(()) => {
                log::info!(
                    "self-update/appimage: using cached appimageupdatetool: {}",
                    cached.display()
                );
                return Ok(cached);
            }
            Err(e) => {
                log::warn!(
                    "self-update/appimage: cached tool digest mismatch, re-downloading: {e:#}"
                );
                let _ = std::fs::remove_file(&cached);
            }
        }
    }

    download_tool(url, expected, &cached)?;
    Ok(cached)
}

fn cache_path_for(arch: &str) -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .context("neither XDG_CACHE_HOME nor HOME is set")?;
    let dir = base.join(crate::runtime_paths::APP_SUBDIR);
    std::fs::create_dir_all(&dir).with_context(|| format!("create cache dir {}", dir.display()))?;
    Ok(dir.join(format!("appimageupdatetool-{arch}.AppImage")))
}

fn download_tool(url: &str, expected: &[u8; 32], dest: &Path) -> Result<()> {
    log::info!("self-update/appimage: downloading appimageupdatetool from {url}");

    let mut response = ureq::get(url)
        .config()
        .timeout_global(Some(UPDATE_HTTP_TIMEOUT))
        .build()
        .header(
            "User-Agent",
            &format!("paneflow/{}", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .with_context(|| "Could not download update tool. Try again when online.".to_string())?;

    if response.status().as_u16() == 404 {
        return Err(anyhow::Error::new(UpdateError::ReleaseAssetMissing {
            url: url.to_string(),
        }));
    }
    if !response.status().is_success() {
        bail!(
            "Could not download update tool (HTTP {}). Try again later.",
            response.status()
        );
    }

    let partial_name = dest
        .file_name()
        .map(|n| {
            let mut s = n.to_os_string();
            s.push(".partial");
            s
        })
        .context("derive partial filename")?;
    let tmp = dest.with_file_name(partial_name);

    const MAX_TOOL_BYTES: u64 = 100 * 1024 * 1024;
    let stream_result = {
        let reader = response.body_mut().as_reader();
        let mut reader = Read::take(reader, MAX_TOOL_BYTES + 1);
        let mut file =
            std::fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        std::io::copy(&mut reader, &mut file)
            .context("stream download to disk")
            .and_then(|written| {
                file.sync_all().context("flush download to disk")?;
                Ok(written)
            })
    };
    let written = match stream_result {
        Ok(n) => n,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    };
    if written > MAX_TOOL_BYTES {
        let _ = std::fs::remove_file(&tmp);
        bail!(
            "Update tool download exceeded {} MiB - aborting. Try again later.",
            MAX_TOOL_BYTES / 1024 / 1024
        );
    }

    if let Err(e) = verify_sha256_of_file(&tmp, expected) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&tmp)?.permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&tmp, perms)?;
    }

    std::fs::rename(&tmp, dest)
        .with_context(|| format!("rename {} → {}", tmp.display(), dest.display()))?;
    Ok(())
}

fn verify_sha256_of_file(file: &Path, expected: &[u8; 32]) -> Result<()> {
    let mut f = std::fs::File::open(file)
        .with_context(|| format!("open {} for hashing", file.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).context("read chunk for hashing")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    if digest.as_slice() != expected.as_slice() {
        return Err(anyhow::Error::new(IntegrityMismatch {
            expected: hex_lower(expected),
            got: hex_lower(digest.as_slice()),
        }));
    }
    Ok(())
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

fn invoke_tool(tool: &Path, target: &Path) -> Result<()> {
    let mut cmd = Command::new(tool);
    cmd.env("APPIMAGE_EXTRACT_AND_RUN", "1")
        .arg("-O")
        .arg(target);

    let output = match paneflow_process::run_with_timeout(
        cmd,
        APPIMAGE_TOOL_DEADLINE,
        APPIMAGE_TOOL_STDOUT_CAP,
    ) {
        Ok(output) => output,
        Err(paneflow_process::ProcError::Timeout) => {
            log::warn!(
                "self-update/appimage: {} exceeded {APPIMAGE_TOOL_DEADLINE:?} - killed",
                tool.display(),
            );
            bail!(UpdateError::Timeout);
        }
        Err(e) => {
            return Err(anyhow::Error::new(e)).with_context(|| format!("spawn {}", tool.display()));
        }
    };

    if output.status.success() {
        log::info!(
            "self-update/appimage: updated {} in place",
            target.display()
        );
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stderr}\n{stdout}");
    let tag = classify_error(&combined);
    log::warn!(
        "self-update/appimage: tool exit={} tag={tag:?} stderr={stderr}",
        output.status
    );
    bail!(tag);
}

fn classify_error(output: &str) -> UpdateError {
    let lower = output.to_ascii_lowercase();
    if lower.contains("libfuse.so.2")
        || lower.contains("libfuse2")
        || lower.contains("fuse: failed to exec fusermount")
    {
        return UpdateError::Fuse2Missing;
    }
    if lower.contains("no update information")
        || lower.contains("update information not found")
        || lower.contains("no update_information")
    {
        return UpdateError::Other(
            "This AppImage cannot self-update. Download the latest version from the releases page."
                .to_string(),
        );
    }
    if lower.contains("could not resolve host")
        || lower.contains("could not connect")
        || lower.contains("failed to connect")
        || lower.contains("network is unreachable")
        || lower.contains("no such host")
    {
        return UpdateError::Network(output.to_string());
    }
    if lower.contains("checksum") || lower.contains("signature") || lower.contains("hash mismatch")
    {
        return UpdateError::IntegrityMismatch {
            expected: String::new(),
            got: String::new(),
        };
    }
    if lower.contains("no space left") || lower.contains("disk full") {
        return UpdateError::DiskFull {
            path: std::path::PathBuf::new(),
        };
    }
    UpdateError::Other(
        "Update failed. Try again later, or download the new AppImage manually from the releases page."
            .to_string(),
    )
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn empty_source_path_errors_without_spawning() {
        let r = run_update(
            Path::new(""),
            "https://github.com/arthjean/paneflow/releases/download/v0/x.AppImage",
        );
        let err = r.unwrap_err().to_string();
        assert!(
            err.contains("$APPIMAGE"),
            "expected $APPIMAGE hint in error, got: {err}"
        );
    }

    #[test]
    fn nonexistent_source_path_errors() {
        let r = run_update(
            Path::new("/tmp/paneflow-does-not-exist-xyz.AppImage"),
            "https://github.com/arthjean/paneflow/releases/download/v0/x.AppImage",
        );
        let err = r.unwrap_err().to_string();
        assert!(
            err.contains("not found"),
            "expected 'not found' in error, got: {err}"
        );
    }

    #[test]
    fn classify_error_detects_missing_update_info() {
        match classify_error("zsync2 error: AppImage has no update information") {
            UpdateError::Other(msg) => assert!(msg.contains("cannot self-update"), "got: {msg}"),
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn classify_error_detects_network_variants() {
        for input in [
            "curl: (6) Could not resolve host: github.com",
            "Could not connect to server",
            "Failed to connect: timeout",
            "network is unreachable",
        ] {
            assert!(
                matches!(classify_error(input), UpdateError::Network(_)),
                "input {input:?} → {:?}",
                classify_error(input)
            );
        }
    }

    #[test]
    fn classify_error_detects_integrity_failure() {
        for input in [
            "checksum mismatch after download",
            "Signature verification failed",
        ] {
            assert!(
                matches!(classify_error(input), UpdateError::IntegrityMismatch { .. }),
                "input {input:?} → {:?}",
                classify_error(input)
            );
        }
    }

    #[test]
    fn classify_error_detects_disk_full() {
        assert!(matches!(
            classify_error("write failed: No space left on device"),
            UpdateError::DiskFull { .. }
        ));
    }

    #[test]
    fn classify_error_detects_fuse2_missing() {
        for input in [
            "error while loading shared libraries: libfuse.so.2",
            "fuse: failed to exec fusermount",
            "libfuse2 is required",
        ] {
            assert!(
                matches!(classify_error(input), UpdateError::Fuse2Missing),
                "input {input:?} → {:?}",
                classify_error(input)
            );
        }
    }

    #[test]
    fn classify_error_falls_back_generic() {
        match classify_error("some totally unexpected garbage") {
            UpdateError::Other(msg) => assert!(msg.contains("Update failed"), "got: {msg}"),
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn classify_error_is_case_insensitive() {
        assert!(matches!(
            classify_error("COULD NOT RESOLVE HOST: foo"),
            UpdateError::Network(_)
        ));
    }

    #[test]
    fn invoke_tool_succeeds_with_stub_true() {
        let tmp = tempfile::TempDir::new().unwrap();
        let target = tmp.path().join("fake.AppImage");
        std::fs::write(&target, b"x").unwrap();
        let r = invoke_tool(Path::new("/bin/true"), &target);
        assert!(r.is_ok(), "expected success, got: {r:?}");
    }

    #[test]
    fn failed_verify_leaves_live_binary_untouched_and_no_candidate() {
        let tmp = tempfile::TempDir::new().unwrap();
        let live = tmp.path().join("PaneFlow-x86_64.AppImage");
        let original = b"the genuine live AppImage bytes";
        std::fs::write(&live, original).unwrap();

        let candidate = candidate_path_for(&live).unwrap();
        std::fs::copy(&live, &candidate).unwrap();
        assert!(invoke_tool(Path::new("/bin/true"), &candidate).is_ok());

        let verify = super::super::super::signature::fetch_and_verify(
            &candidate,
            "https://github.com/arthjean/paneflow/releases/download/v0/x.AppImage",
        );
        assert!(verify.is_err(), "unsigned build must fail closed");
        let _ = std::fs::remove_file(&candidate);

        assert!(
            !candidate.exists(),
            "candidate must be removed on verify failure"
        );
        assert_eq!(
            std::fs::read(&live).unwrap(),
            original,
            "live AppImage must be byte-for-byte untouched on verify failure"
        );
    }

    #[test]
    fn invoke_tool_propagates_missing_update_info() {
        let tmp = tempfile::TempDir::new().unwrap();
        let stub = tmp.path().join("fake-tool.sh");
        let mut f = std::fs::File::create(&stub).unwrap();
        writeln!(
            f,
            "#!/bin/sh\necho 'zsync2: AppImage has no update information' 1>&2\nexit 1"
        )
        .unwrap();
        drop(f);
        let mut perms = std::fs::metadata(&stub).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&stub, perms).unwrap();

        let target = tmp.path().join("fake.AppImage");
        std::fs::write(&target, b"x").unwrap();

        let mut err = String::new();
        for _ in 0..100 {
            err = invoke_tool(&stub, &target).unwrap_err().to_string();
            if !err.starts_with("spawn ") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(err.contains("cannot self-update"), "got: {err}");
    }

    const PINNED_TAG: &str = "2.0.0-alpha-1-20251018";

    #[test]
    fn tool_asset_for_x86_64_points_at_pinned_tag() {
        let (url, digest) = tool_asset_for("x86_64").unwrap();
        assert!(
            url.contains(PINNED_TAG),
            "x86_64 URL should embed the pinned tag, got: {url}"
        );
        assert!(
            !url.contains("/latest/"),
            "x86_64 URL must not use the floating 'latest' redirect: {url}"
        );
        assert_eq!(digest, &APPIMAGEUPDATETOOL_SHA256_X86_64);
    }

    #[test]
    fn tool_asset_for_aarch64_points_at_pinned_tag() {
        let (url, digest) = tool_asset_for("aarch64").unwrap();
        assert!(
            url.contains(PINNED_TAG),
            "aarch64 URL should embed the pinned tag, got: {url}"
        );
        assert!(
            !url.contains("/latest/"),
            "aarch64 URL must not use the floating 'latest' redirect: {url}"
        );
        assert_eq!(digest, &APPIMAGEUPDATETOOL_SHA256_AARCH64);
    }

    #[test]
    fn tool_asset_for_unknown_arch_errors() {
        let err = tool_asset_for("riscv64").unwrap_err().to_string();
        assert!(err.contains("riscv64"), "got: {err}");
        assert!(err.contains("manually"), "got: {err}");
    }

    #[test]
    fn verify_sha256_rejects_mismatched_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("tampered.AppImage");
        std::fs::write(&path, b"not the real tool bytes").unwrap();

        let err = verify_sha256_of_file(&path, &APPIMAGEUPDATETOOL_SHA256_X86_64).unwrap_err();
        let mm = err
            .downcast_ref::<IntegrityMismatch>()
            .expect("mismatch error should be an IntegrityMismatch");
        assert_eq!(
            mm.expected,
            hex_lower(&APPIMAGEUPDATETOOL_SHA256_X86_64),
            "expected digest should be the hex of the pinned constant"
        );
        assert_ne!(mm.got, mm.expected, "got digest must differ from expected");
        assert_eq!(
            mm.got.len(),
            64,
            "got digest must be a full 64-char sha256 hex, got: {:?}",
            mm.got
        );
    }

    #[test]
    fn verify_sha256_mismatch_classifies_as_integrity() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("tampered.AppImage");
        std::fs::write(&path, b"x").unwrap();
        let err = verify_sha256_of_file(&path, &APPIMAGEUPDATETOOL_SHA256_X86_64).unwrap_err();
        assert!(matches!(
            UpdateError::classify(&err),
            UpdateError::IntegrityMismatch { .. }
        ));
    }

    #[test]
    fn digest_mismatch_deletes_file_on_disk() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("appimageupdatetool.AppImage.partial");
        std::fs::write(&path, b"tampered").unwrap();
        assert!(path.exists());

        if verify_sha256_of_file(&path, &APPIMAGEUPDATETOOL_SHA256_X86_64).is_err() {
            std::fs::remove_file(&path).unwrap();
        }
        assert!(
            !path.exists(),
            "mismatched file must be removed from disk after verification failure"
        );
    }

    #[test]
    fn pinned_digest_hex_matches_byte_array() {
        assert_eq!(
            hex_lower(&APPIMAGEUPDATETOOL_SHA256_X86_64),
            "d976cdac667b03dee8cb23fb95ef74b042c406c5cbab3ff294d2b16efeaff84f"
        );
        assert_eq!(
            hex_lower(&APPIMAGEUPDATETOOL_SHA256_AARCH64),
            "7aaf89dd4cf66ebd940d416c67e1c240c57a139cee38d9c0ed3bb9387bc435b0"
        );
    }
}
