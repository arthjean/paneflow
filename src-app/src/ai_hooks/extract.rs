use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow};
use sha2::{Digest, Sha256};

use crate::assets::Bins;

const TARGET_TRIPLE: &str = env!("PANEFLOW_TARGET_TRIPLE");

#[cfg(any(not(windows), debug_assertions))]
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn extract_plan() -> Vec<(&'static str, &'static str)> {
    let mut plan: Vec<(&'static str, &'static str)> = crate::agent_launcher::TerminalAgent::ALL
        .iter()
        .map(|agent| (agent.binary(), "paneflow-shim"))
        .collect();
    plan.push(("paneflow-ai-hook", "paneflow-ai-hook"));
    plan
}

#[inline]
fn exe_suffix() -> &'static str {
    if cfg!(windows) { ".exe" } else { "" }
}

fn embedded_bytes(name: &str) -> Result<std::borrow::Cow<'static, [u8]>> {
    let key = format!("bin/{TARGET_TRIPLE}/{name}");
    Bins::get(&key)
        .map(|f| f.data)
        .ok_or_else(|| anyhow!("US-008: embed entry {key} missing - did build.rs stage it?"))
}

pub(crate) struct Entry<'a> {
    pub filename: String,
    pub bytes: &'a [u8],
}

static VERIFIED_BIN_DIR: OnceLock<PathBuf> = OnceLock::new();

fn memoized_verified_path(
    cache: &OnceLock<PathBuf>,
    verify: impl FnOnce() -> Result<PathBuf>,
) -> Result<PathBuf> {
    if let Some(path) = cache.get() {
        return Ok(path.clone());
    }

    let path = verify()?;
    Ok(cache.get_or_init(|| path).clone())
}

pub fn ensure_binaries_extracted() -> Result<PathBuf> {
    memoized_verified_path(&VERIFIED_BIN_DIR, ensure_binaries_extracted_uncached)
}

fn ensure_binaries_extracted_uncached() -> Result<PathBuf> {
    #[cfg(windows)]
    if let Some(dir) = packaged_bin_dir_if_complete() {
        return Ok(dir);
    }

    #[cfg(all(windows, not(debug_assertions)))]
    {
        Err(anyhow!(
            "US-008: packaged Windows helper dir missing or incomplete; refusing to execute helper binaries from the per-user cache"
        ))
    }

    #[cfg(any(not(windows), debug_assertions))]
    {
        let cache_root = dirs::cache_dir()
            .ok_or_else(|| anyhow!("US-008: dirs::cache_dir() returned None; cannot extract"))?;
        let target_dir = cache_root
            .join(crate::runtime_paths::APP_SUBDIR)
            .join("bin")
            .join(VERSION);

        let suffix = exe_suffix();
        let plan = extract_plan();
        let mut buffers: Vec<(String, std::borrow::Cow<'static, [u8]>)> =
            Vec::with_capacity(plan.len());
        for (out_name, src_name) in plan {
            let src_full = format!("{src_name}{suffix}");
            let out_full = format!("{out_name}{suffix}");
            buffers.push((out_full, embedded_bytes(&src_full)?));
        }
        let entries: Vec<Entry<'_>> = buffers
            .iter()
            .map(|(n, b)| Entry {
                filename: n.clone(),
                bytes: b.as_ref(),
            })
            .collect();

        extract_into(&entries, &target_dir)?;
        Ok(target_dir)
    }
}

#[cfg(windows)]
fn packaged_bin_dir_if_complete() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.join("bin");
    let suffix = exe_suffix();
    for (out_name, _) in extract_plan() {
        if !dir.join(format!("{out_name}{suffix}")).is_file() {
            return None;
        }
    }
    Some(dir)
}

pub fn ensure_bridge_extracted() -> Result<PathBuf> {
    let bridge_path = crate::runtime_paths::bridge_binary_path().ok_or_else(|| {
        anyhow!("EP-001 US-003: data_dir() unresolvable/unwritable; cannot extract paneflow-mcp")
    })?;
    let target_dir = bridge_path
        .parent()
        .ok_or_else(|| {
            anyhow!(
                "EP-001 US-003: bridge path {} has no parent",
                bridge_path.display()
            )
        })?
        .to_path_buf();
    let filename = bridge_path
        .file_name()
        .ok_or_else(|| {
            anyhow!(
                "EP-001 US-003: bridge path {} has no filename",
                bridge_path.display()
            )
        })?
        .to_string_lossy()
        .into_owned();

    let bytes = embedded_bytes(&filename)?;
    let entry = Entry {
        filename,
        bytes: bytes.as_ref(),
    };
    extract_into(std::slice::from_ref(&entry), &target_dir)?;
    Ok(bridge_path)
}

pub fn ensure_ai_hook_extracted() -> Result<PathBuf> {
    let hook_path = crate::runtime_paths::ai_hook_binary_path().ok_or_else(|| {
        anyhow!(
            "EP-004 US-016: data_dir() unresolvable/unwritable; cannot extract paneflow-ai-hook"
        )
    })?;
    let target_dir = hook_path
        .parent()
        .ok_or_else(|| {
            anyhow!(
                "EP-004 US-016: ai-hook path {} has no parent",
                hook_path.display()
            )
        })?
        .to_path_buf();
    let filename = hook_path
        .file_name()
        .ok_or_else(|| {
            anyhow!(
                "EP-004 US-016: ai-hook path {} has no filename",
                hook_path.display()
            )
        })?
        .to_string_lossy()
        .into_owned();

    let bytes = embedded_bytes(&filename)?;
    let entry = Entry {
        filename,
        bytes: bytes.as_ref(),
    };
    extract_into(std::slice::from_ref(&entry), &target_dir)?;
    Ok(hook_path)
}

pub(crate) fn extract_into(entries: &[Entry<'_>], target_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(target_dir)
        .with_context(|| format!("US-008: create cache dir {} failed", target_dir.display()))?;

    for entry in entries {
        if entry.filename.contains('/')
            || entry.filename.contains('\\')
            || entry.filename == ".."
            || entry.filename == "."
            || entry.filename.is_empty()
        {
            return Err(anyhow!(
                "US-008: refusing to extract entry with non-basename filename {:?}",
                entry.filename
            ));
        }
        let final_path = target_dir.join(&entry.filename);

        if file_matches_digest(&final_path, entry.bytes)? {
            continue;
        }

        write_atomic(&final_path, entry.bytes)
            .with_context(|| format!("US-008: atomic write of {} failed", final_path.display()))?;

        if !file_matches_digest(&final_path, entry.bytes)? {
            return Err(anyhow!(
                "US-008: post-write digest mismatch for {} - \
                 filesystem or AV interference suspected",
                final_path.display()
            ));
        }
    }

    Ok(())
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().into()
}

fn file_matches_digest(path: &Path, expected: &[u8]) -> Result<bool> {
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            return Err(anyhow::Error::new(e))
                .with_context(|| format!("US-008: open {} failed", path.display()));
        }
    };

    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = std::io::Read::read(&mut file, &mut buf)
            .with_context(|| format!("US-008: read {} failed", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual: [u8; 32] = hasher.finalize().into();
    Ok(actual == sha256(expected))
}

fn write_atomic(final_path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = final_path
        .parent()
        .ok_or_else(|| anyhow!("US-008: {} has no parent dir", final_path.display()))?;

    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("US-008: tempfile in {} failed", parent.display()))?;
    tmp.write_all(bytes)
        .context("US-008: write_all to tempfile failed")?;
    tmp.as_file_mut()
        .sync_all()
        .context("US-008: sync_all on tempfile failed")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(tmp.path(), perms)
            .with_context(|| format!("US-008: chmod 0o755 on {} failed", tmp.path().display()))?;
    }

    persist_atomic(tmp, final_path)
}

fn persist_atomic(tmp: tempfile::NamedTempFile, final_path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        const MAX_ATTEMPTS: u32 = 10;
        const BACKOFF: std::time::Duration = std::time::Duration::from_millis(50);
        let mut tmp = tmp;
        let mut attempt: u32 = 0;
        loop {
            match tmp.persist(final_path) {
                Ok(_) => return Ok(()),
                Err(e) => {
                    attempt += 1;
                    let transient = matches!(e.error.raw_os_error(), Some(5) | Some(32));
                    if transient && attempt < MAX_ATTEMPTS {
                        tmp = e.file;
                        std::thread::sleep(BACKOFF);
                        continue;
                    }
                    return Err(anyhow!(
                        "US-008: atomic rename {} -> {} failed after {attempt} attempt(s): {}",
                        e.file.path().display(),
                        final_path.display(),
                        e.error
                    ));
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        tmp.persist(final_path).map_err(|e| {
            anyhow!(
                "US-008: atomic rename {} -> {} failed: {}",
                e.file.path().display(),
                final_path.display(),
                e.error
            )
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE_SHIM: &[u8] = b"paneflow-shim synthetic bytes v0";
    const FAKE_HOOK: &[u8] = b"paneflow-ai-hook synthetic bytes v0";

    fn synthetic_entries() -> Vec<Entry<'static>> {
        let suffix = exe_suffix();
        vec![
            Entry {
                filename: format!("claude{suffix}"),
                bytes: FAKE_SHIM,
            },
            Entry {
                filename: format!("codex{suffix}"),
                bytes: FAKE_SHIM,
            },
            Entry {
                filename: format!("paneflow-ai-hook{suffix}"),
                bytes: FAKE_HOOK,
            },
        ]
    }

    #[test]
    fn extracts_all_three_filenames() {
        let dir = tempfile::TempDir::new().unwrap();
        let entries = synthetic_entries();
        extract_into(&entries, dir.path()).unwrap();

        let suffix = exe_suffix();
        for expected in [
            format!("claude{suffix}"),
            format!("codex{suffix}"),
            format!("paneflow-ai-hook{suffix}"),
        ] {
            let p = dir.path().join(&expected);
            assert!(
                p.is_file(),
                "US-008 AC: expected {} to exist after extraction",
                p.display()
            );
        }
    }

    #[test]
    fn extracted_bytes_match_input_sha256() {
        let dir = tempfile::TempDir::new().unwrap();
        let entries = synthetic_entries();
        extract_into(&entries, dir.path()).unwrap();

        for entry in &entries {
            let p = dir.path().join(&entry.filename);
            let on_disk = std::fs::read(&p).unwrap();
            assert_eq!(
                sha256(&on_disk),
                sha256(entry.bytes),
                "US-008 AC: extracted {} must SHA256-match the input bytes",
                p.display()
            );
        }
    }

    #[test]
    fn shim_copies_are_identical() {
        let dir = tempfile::TempDir::new().unwrap();
        let entries = synthetic_entries();
        extract_into(&entries, dir.path()).unwrap();

        let suffix = exe_suffix();
        let claude = std::fs::read(dir.path().join(format!("claude{suffix}"))).unwrap();
        let codex = std::fs::read(dir.path().join(format!("codex{suffix}"))).unwrap();
        assert_eq!(
            claude, codex,
            "US-008 AC: claude and codex are both copies of paneflow-shim"
        );
    }

    #[test]
    fn re_extraction_is_noop() {
        let dir = tempfile::TempDir::new().unwrap();
        let entries = synthetic_entries();
        extract_into(&entries, dir.path()).unwrap();

        let mut first_mtimes = Vec::new();
        for entry in &entries {
            let p = dir.path().join(&entry.filename);
            first_mtimes.push((
                p.clone(),
                std::fs::metadata(&p).unwrap().modified().unwrap(),
            ));
        }

        std::thread::sleep(std::time::Duration::from_millis(50));

        extract_into(&entries, dir.path()).unwrap();

        for (p, first_mtime) in first_mtimes {
            let second_mtime = std::fs::metadata(&p).unwrap().modified().unwrap();
            assert_eq!(
                first_mtime,
                second_mtime,
                "US-008 AC: re-extraction of unchanged bytes must be a no-op (mtime unchanged) for {}",
                p.display()
            );
        }
    }

    #[test]
    fn successful_verification_is_memoized_for_process_lifetime() {
        let cache = OnceLock::new();
        let calls = std::cell::Cell::new(0);
        let expected = PathBuf::from("verified-bin-dir");

        for _ in 0..2 {
            let actual = memoized_verified_path(&cache, || {
                calls.set(calls.get() + 1);
                Ok(expected.clone())
            })
            .unwrap();
            assert_eq!(actual, expected);
        }

        assert_eq!(calls.get(), 1, "a verified directory must be reused");
    }

    #[test]
    fn failed_verification_is_retried() {
        let cache = OnceLock::new();
        let calls = std::cell::Cell::new(0);

        for _ in 0..2 {
            let result = memoized_verified_path(&cache, || {
                calls.set(calls.get() + 1);
                Err(anyhow!("transient extraction failure"))
            });
            assert!(result.is_err());
        }

        assert_eq!(calls.get(), 2, "a failed verification must not be cached");
        assert!(cache.get().is_none());
    }

    #[test]
    fn stale_bytes_are_overwritten() {
        let dir = tempfile::TempDir::new().unwrap();
        let entries = synthetic_entries();

        let stale_path = dir.path().join(&entries[0].filename);
        std::fs::write(&stale_path, b"stale").unwrap();

        extract_into(&entries, dir.path()).unwrap();

        let after = std::fs::read(&stale_path).unwrap();
        assert_eq!(
            after, entries[0].bytes,
            "US-008: stale bytes must be overwritten by the current embed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_mode_is_0o755() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let entries = synthetic_entries();
        extract_into(&entries, dir.path()).unwrap();

        for entry in &entries {
            let p = dir.path().join(&entry.filename);
            let mode = std::fs::metadata(&p).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o777,
                0o755,
                "US-008 AC: {} must be mode 0o755 on Unix, got 0o{:o}",
                p.display(),
                mode & 0o777
            );
        }
    }

    #[test]
    fn bins_embed_contains_all_staged_binaries() {
        let suffix = exe_suffix();
        for src in ["paneflow-shim", "paneflow-ai-hook", "paneflow-mcp"] {
            let name = format!("{src}{suffix}");
            let bytes = embedded_bytes(&name).unwrap_or_else(|e| {
                panic!("US-008/EP-001: Bins must contain `bin/{TARGET_TRIPLE}/{name}`: {e}")
            });
            assert!(
                !bytes.is_empty(),
                "US-008/EP-001: embedded {name} must be non-empty"
            );
        }
    }

    #[test]
    fn ensure_binaries_extracted_produces_all_agent_wrappers() {
        if dirs::cache_dir().is_none() {
            eprintln!("skip: dirs::cache_dir() unresolvable in this environment");
            return;
        }
        let dir = ensure_binaries_extracted().unwrap();
        let suffix = exe_suffix();
        let mut expected: Vec<String> = crate::agent_launcher::TerminalAgent::ALL
            .iter()
            .map(|a| format!("{}{suffix}", a.binary()))
            .collect();
        expected.push(format!("paneflow-ai-hook{suffix}"));
        for name in expected {
            let p = dir.join(&name);
            assert!(
                p.is_file(),
                "US-008: ensure_binaries_extracted must produce {}",
                p.display()
            );
        }
    }

    #[test]
    fn wrapped_stems_match_shim_detect_list() {
        let binaries: Vec<&str> = crate::agent_launcher::TerminalAgent::ALL
            .iter()
            .map(|a| a.binary())
            .collect();
        assert_eq!(
            binaries,
            vec![
                "claude",
                "codex",
                "opencode",
                "pi",
                "hermes",
                "grok",
                "amp",
                "cursor-agent",
                "gemini",
                "kiro-cli",
                "agy",
                "copilot",
                "codebuddy",
                "droid",
                "qodercli",
                "openclaw",
            ],
        );
    }

    #[test]
    fn ensure_bridge_extracted_produces_stable_path() {
        if crate::runtime_paths::bridge_binary_path().is_none() {
            eprintln!("skip: bridge_binary_path() unresolvable in this environment");
            return;
        }
        let path = ensure_bridge_extracted().unwrap();
        assert!(
            path.is_file(),
            "EP-001 US-003: ensure_bridge_extracted must produce {}",
            path.display()
        );
        let suffix = exe_suffix();
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            format!("paneflow-mcp{suffix}"),
            "EP-001 US-003: bridge filename must be paneflow-mcp[.exe]"
        );
    }

    #[test]
    fn bridge_path_is_non_versioned_and_distinct_from_cache() {
        let Some(bridge) = crate::runtime_paths::bridge_binary_path() else {
            eprintln!("skip: bridge_binary_path() unresolvable in this environment");
            return;
        };
        let bridge_str = bridge.to_string_lossy();
        let version = env!("CARGO_PKG_VERSION");
        assert!(
            !bridge_str.contains(version),
            "EP-001 US-003: bridge path {bridge_str} must NOT embed the version {version}"
        );
        if let Ok(cache) = ensure_binaries_extracted() {
            assert_ne!(
                bridge.parent(),
                Some(cache.as_path()),
                "EP-001 US-003: bridge dir must differ from the versioned cache dir"
            );
        }
    }

    #[test]
    fn rejects_non_basename_filenames() {
        let dir = tempfile::TempDir::new().unwrap();
        let bad_cases: &[&str] = &["..", ".", "", "nested/evil", "..\\evil"];
        for bad in bad_cases {
            let entries = [Entry {
                filename: (*bad).to_string(),
                bytes: b"x",
            }];
            let err = extract_into(&entries, dir.path())
                .err()
                .unwrap_or_else(|| panic!("US-008: {bad:?} must be rejected"));
            assert!(
                err.to_string().contains("non-basename"),
                "US-008: rejection for {bad:?} must mention 'non-basename'; got {err}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn returns_err_when_parent_is_readonly() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let ro_parent = dir.path().join("ro");
        std::fs::create_dir(&ro_parent).unwrap();
        std::fs::set_permissions(&ro_parent, std::fs::Permissions::from_mode(0o555)).unwrap();

        let target = ro_parent.join("bin");
        let entries = synthetic_entries();
        let res = extract_into(&entries, &target);

        std::fs::set_permissions(&ro_parent, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(
            res.is_err(),
            "US-008 AC: extraction into a read-only parent must return Err"
        );
    }
}
