use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

type BuildResult<T> = Result<T, Box<dyn Error>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum NativePlatform {
    Linux,
    Windows,
}

struct TargetSpec {
    platform: NativePlatform,
    archive_path_key: &'static str,
    archive_hash_key: &'static str,
    normalization_key: &'static str,
    zig_target: &'static str,
    link_name: &'static str,
    system_libraries: &'static [&'static str],
}

fn main() -> BuildResult<()> {
    println!("cargo:rerun-if-env-changed=PANEFLOW_LIBGHOSTTY_DIR");
    let crate_dir = PathBuf::from(required_env("CARGO_MANIFEST_DIR")?);
    let workspace = crate_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| build_error("the -sys crate must live under <workspace>/crates"))?
        .to_path_buf();
    let manifest_path = workspace.join("native/libghostty/manifest.toml");
    let manifest = fs::read_to_string(&manifest_path).map_err(|error| {
        build_error(format!("cannot read {}: {error}", manifest_path.display()))
    })?;
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    println!(
        "cargo:rerun-if-changed={}",
        workspace.join("native/libghostty/bindings.rs").display()
    );
    println!(
        "cargo:rustc-env=PANEFLOW_GHOSTTY_API_VERSION={}",
        manifest_value(&manifest, "api_version")?
    );
    println!(
        "cargo:rustc-env=PANEFLOW_GHOSTTY_APP_VERSION={}",
        manifest_value(&manifest, "ghostty_app_version")?
    );
    if std::env::var_os("CARGO_FEATURE_LINK").is_none() {
        return Ok(());
    }

    let target = required_env("TARGET")?;
    let Some(spec) = target_spec(&target)? else {
        return Ok(());
    };
    let action = corrective_action(spec.platform, &target);

    let canonical_bindings = workspace.join(manifest_value(&manifest, "bindings_path")?);
    verify_text_hash(
        &canonical_bindings,
        manifest_value(&manifest, "bindings_sha256")?,
        &target,
        &action,
    )?;

    let bundled = workspace.join("native/libghostty/prebuilt").join(&target);
    let (prepared, uses_bundled_archive) = match std::env::var_os("PANEFLOW_LIBGHOSTTY_DIR") {
        Some(path) => (PathBuf::from(path), false),
        None => (bundled, true),
    };
    let archive = prepared.join(manifest_value(&manifest, spec.archive_path_key)?);
    let header = prepared.join("include/ghostty/vt.h");
    let bindings = prepared.join("bindings.rs");
    let build_info = prepared.join("build-info.txt");
    let headers_index = prepared.join("headers.sha256");
    let symbols = prepared.join("symbols.txt");

    let mut required_inputs = vec![
        archive.as_path(),
        header.as_path(),
        bindings.as_path(),
        build_info.as_path(),
    ];
    if spec.platform == NativePlatform::Windows {
        required_inputs.extend([headers_index.as_path(), symbols.as_path()]);
        println!("cargo:rerun-if-changed={}", prepared.display());
    }
    for path in required_inputs {
        require_file(path, &target, &action)?;
        println!("cargo:rerun-if-changed={}", path.display());
    }

    verify_text_hash(
        &header,
        manifest_value(&manifest, "header_sha256")?,
        &target,
        &action,
    )?;
    verify_text_hash(
        &bindings,
        manifest_value(&manifest, "bindings_sha256")?,
        &target,
        &action,
    )?;

    let info = fs::read_to_string(&build_info).map_err(|error| {
        artifact_error(
            &target,
            &build_info,
            format!("cannot read build metadata: {error}"),
            &action,
        )
    })?;
    let normalization = manifest_value(&manifest, spec.normalization_key)?;
    let canonical_source = manifest_value(&manifest, "windows_canonical_source_path")?;
    let canonical_cache = format!("{canonical_source}/.paneflow-zig-cache");
    let canonical_prefix = format!("{canonical_source}/.paneflow-zig-output");
    let mut expected_info = vec![
        ("source_sha", manifest_value(&manifest, "source_sha")?),
        ("zig_version", manifest_value(&manifest, "zig_version")?),
        ("header_sha256", manifest_value(&manifest, "header_sha256")?),
        (
            "bindings_sha256",
            manifest_value(&manifest, "bindings_sha256")?,
        ),
        ("rust_target", target.as_str()),
        ("zig_target", spec.zig_target),
        ("optimize", manifest_value(&manifest, "build_mode")?),
        ("archive_normalization", normalization),
    ];
    if spec.platform == NativePlatform::Linux {
        expected_info.push((
            "build_info_symbol",
            manifest_value(&manifest, "build_info_symbol")?,
        ));
    } else {
        expected_info.extend([
            (
                "source_date_epoch",
                manifest_value(&manifest, "windows_source_date_epoch")?,
            ),
            ("simd", "true"),
            (
                "build_seed",
                manifest_value(&manifest, "windows_build_seed")?,
            ),
            (
                "build_jobs",
                manifest_value(&manifest, "windows_build_jobs")?,
            ),
            ("canonical_source_path", canonical_source),
            ("canonical_cache_path", canonical_cache.as_str()),
            ("canonical_prefix_path", canonical_prefix.as_str()),
            (
                "msvc_toolset",
                manifest_value(&manifest, "windows_msvc_toolset")?,
            ),
            ("windows_sdk", manifest_value(&manifest, "windows_sdk")?),
            (
                "llvm_version",
                manifest_value(&manifest, "windows_llvm_version")?,
            ),
            ("crt", manifest_value(&manifest, "windows_crt")?),
            (
                "cxx_runtime",
                manifest_value(&manifest, "windows_cxx_runtime")?,
            ),
            ("system_libraries", "ntdll.lib,kernel32.lib"),
        ]);
    }
    for (key, expected) in expected_info {
        let actual = info_value(&info, key).map_err(|error| {
            artifact_error(
                &target,
                &build_info,
                format!("incoherent build metadata: {error}"),
                &action,
            )
        })?;
        if actual != expected {
            return Err(artifact_error(
                &target,
                &build_info,
                format!("build metadata `{key}` is `{actual}`, expected `{expected}`"),
                &action,
            ));
        }
    }

    let expected_archive_hash = manifest_value(&manifest, spec.archive_hash_key)?;
    let prepared_archive_hash =
        artifact_info_value(&info, "archive_sha256", &build_info, &target, &action)?;
    if (uses_bundled_archive || spec.platform == NativePlatform::Windows)
        && prepared_archive_hash != expected_archive_hash
    {
        return Err(artifact_error(
            &target,
            &build_info,
            format!(
                "archive checksum metadata is `{prepared_archive_hash}`, expected `{expected_archive_hash}`"
            ),
            &action,
        ));
    }
    let archive_hash = if uses_bundled_archive || spec.platform == NativePlatform::Windows {
        expected_archive_hash
    } else {
        prepared_archive_hash
    };
    verify_hash(&archive, archive_hash, &target, &action)?;

    if spec.platform == NativePlatform::Windows {
        verify_windows_metadata(&manifest, &info, &headers_index, &symbols, &target, &action)?;
    }

    let link_dir = archive
        .parent()
        .ok_or_else(|| build_error("archive path must have a parent"))?;
    println!("cargo:rustc-link-search=native={}", link_dir.display());
    println!("cargo:rustc-link-lib=static={}", spec.link_name);
    for library in spec.system_libraries {
        println!("cargo:rustc-link-lib=dylib={library}");
    }
    Ok(())
}

fn target_spec(target: &str) -> BuildResult<Option<TargetSpec>> {
    let spec = match target {
        "x86_64-unknown-linux-gnu" => TargetSpec {
            platform: NativePlatform::Linux,
            archive_path_key: "archive_path",
            archive_hash_key: "archive_sha256_x86_64_unknown_linux_gnu",
            normalization_key: "archive_normalization",
            zig_target: "x86_64-linux-gnu",
            link_name: "ghostty-vt",
            system_libraries: &[],
        },
        "aarch64-unknown-linux-gnu" => TargetSpec {
            platform: NativePlatform::Linux,
            archive_path_key: "archive_path",
            archive_hash_key: "archive_sha256_aarch64_unknown_linux_gnu",
            normalization_key: "archive_normalization",
            zig_target: "aarch64-linux-gnu",
            link_name: "ghostty-vt",
            system_libraries: &[],
        },
        "x86_64-pc-windows-msvc" => TargetSpec {
            platform: NativePlatform::Windows,
            archive_path_key: "archive_path_windows",
            archive_hash_key: "archive_sha256_x86_64_pc_windows_msvc",
            normalization_key: "archive_normalization_windows",
            zig_target: "x86_64-windows-msvc",
            link_name: "ghostty-vt-static",
            system_libraries: &["ntdll", "kernel32"],
        },
        unsupported if unsupported.contains("-linux-") => {
            return Err(build_error(format!(
                "libghostty has no reviewed static archive for Linux target {unsupported}"
            )));
        }
        unsupported if unsupported.contains("-windows-") => {
            return Err(build_error(format!(
                "libghostty has no reviewed static archive for Windows target {unsupported}"
            )));
        }
        _ => return Ok(None),
    };
    Ok(Some(spec))
}

fn verify_windows_metadata(
    manifest: &str,
    info: &str,
    headers_index: &Path,
    symbols: &Path,
    target: &str,
    action: &str,
) -> BuildResult<()> {
    let build_info_path = headers_index
        .parent()
        .ok_or_else(|| build_error("headers index path must have a parent"))?
        .join("build-info.txt");
    for (path, manifest_key, info_key) in [
        (
            headers_index,
            "headers_index_sha256_x86_64_pc_windows_msvc",
            "headers_sha256",
        ),
        (
            symbols,
            "symbols_sha256_x86_64_pc_windows_msvc",
            "symbols_sha256",
        ),
    ] {
        let expected = manifest_value(manifest, manifest_key)?;
        let recorded = artifact_info_value(info, info_key, &build_info_path, target, action)?;
        if recorded != expected {
            return Err(artifact_error(
                target,
                path,
                format!("build metadata hash is `{recorded}`, expected `{expected}`"),
                action,
            ));
        }
        verify_text_hash(path, expected, target, action)?;
    }

    let expected_build_info = manifest_value(manifest, "build_info_sha256_x86_64_pc_windows_msvc")?;
    verify_text_hash(&build_info_path, expected_build_info, target, action)?;
    let prepared_root = build_info_path
        .parent()
        .ok_or_else(|| build_error("build-info path must have a parent"))?;
    let indexed_headers = verify_header_index(
        &prepared_root.join("include"),
        headers_index,
        target,
        action,
    )?;
    verify_windows_inventory(prepared_root, &indexed_headers, target, action)?;

    let symbol_count = fs::read_to_string(symbols)
        .map_err(|error| artifact_error(target, symbols, error.to_string(), action))?
        .lines()
        .filter(|line| !line.is_empty())
        .count()
        .to_string();
    if artifact_info_value(info, "symbol_count", &build_info_path, target, action)? != symbol_count
    {
        return Err(artifact_error(
            target,
            symbols,
            format!("symbol inventory count does not match build-info ({symbol_count})"),
            action,
        ));
    }
    Ok(())
}

fn verify_header_index(
    include_root: &Path,
    index: &Path,
    target: &str,
    action: &str,
) -> BuildResult<HashSet<PathBuf>> {
    let contents = fs::read_to_string(index)
        .map_err(|error| artifact_error(target, index, error.to_string(), action))?;
    let mut indexed = HashSet::new();
    for line in contents.lines().filter(|line| !line.is_empty()) {
        let (expected, relative) = line.split_once("  ").ok_or_else(|| {
            artifact_error(
                target,
                index,
                format!("invalid header index line `{line}`"),
                action,
            )
        })?;
        let relative_path = Path::new(relative);
        if expected.len() != 64
            || relative_path.is_absolute()
            || relative_path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(artifact_error(
                target,
                index,
                format!("unsafe header index entry `{line}`"),
                action,
            ));
        }
        if !indexed.insert(relative_path.to_path_buf()) {
            return Err(artifact_error(
                target,
                index,
                format!("duplicate header index entry `{relative}`"),
                action,
            ));
        }
        let header = include_root.join(relative_path);
        require_file(&header, target, action)?;
        verify_text_hash(&header, expected, target, action)?;
    }
    Ok(indexed)
}

fn verify_windows_inventory(
    prepared_root: &Path,
    indexed_headers: &HashSet<PathBuf>,
    target: &str,
    action: &str,
) -> BuildResult<()> {
    let mut expected = HashSet::from([
        PathBuf::from("bindings.rs"),
        PathBuf::from("build-info.txt"),
        PathBuf::from("headers.sha256"),
        PathBuf::from("symbols.txt"),
        PathBuf::from("lib/ghostty-vt-static.lib"),
    ]);
    expected.extend(
        indexed_headers
            .iter()
            .map(|relative| PathBuf::from("include").join(relative)),
    );

    let mut actual = HashSet::new();
    collect_artifact_files(prepared_root, prepared_root, &mut actual, target, action)?;
    if actual == expected {
        return Ok(());
    }

    let mut missing = expected
        .difference(&actual)
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let mut extra = actual
        .difference(&expected)
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    missing.sort_unstable();
    extra.sort_unstable();
    Err(artifact_error(
        target,
        prepared_root,
        format!(
            "artifact inventory mismatch: missing [{}], extra [{}]",
            missing.join(", "),
            extra.join(", ")
        ),
        action,
    ))
}

fn collect_artifact_files(
    root: &Path,
    directory: &Path,
    files: &mut HashSet<PathBuf>,
    target: &str,
    action: &str,
) -> BuildResult<()> {
    let entries = fs::read_dir(directory).map_err(|error| {
        artifact_error(
            target,
            directory,
            format!("cannot enumerate artifact: {error}"),
            action,
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            artifact_error(
                target,
                directory,
                format!("cannot enumerate artifact entry: {error}"),
                action,
            )
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            artifact_error(
                target,
                &path,
                format!("cannot inspect artifact entry: {error}"),
                action,
            )
        })?;
        if file_type.is_dir() {
            collect_artifact_files(root, &path, files, target, action)?;
        } else if file_type.is_file() {
            let relative = path.strip_prefix(root).map_err(|error| {
                artifact_error(
                    target,
                    &path,
                    format!("artifact escaped its prepared root: {error}"),
                    action,
                )
            })?;
            files.insert(relative.to_path_buf());
        } else {
            return Err(artifact_error(
                target,
                &path,
                "artifact contains a symlink or unsupported filesystem entry",
                action,
            ));
        }
    }
    Ok(())
}

fn corrective_action(platform: NativePlatform, target: &str) -> String {
    match platform {
        NativePlatform::Linux => format!(
            "restore native/libghostty/prebuilt/{target}, or run scripts/build-libghostty-linux.sh --target {target} and set PANEFLOW_LIBGHOSTTY_DIR to its output; Cargo performs no downloads"
        ),
        NativePlatform::Windows => format!(
            "restore native/libghostty/prebuilt/{target}, or run scripts/build-libghostty-windows.ps1 -VerifyReproducible; Cargo performs no downloads"
        ),
    }
}

fn require_file(path: &Path, target: &str, action: &str) -> BuildResult<()> {
    if path.is_file() {
        Ok(())
    } else {
        Err(artifact_error(
            target,
            path,
            "required input is missing",
            action,
        ))
    }
}

fn required_env(key: &str) -> BuildResult<String> {
    std::env::var(key).map_err(|_| build_error(format!("Cargo did not set {key}")))
}

fn manifest_value<'a>(manifest: &'a str, key: &str) -> BuildResult<&'a str> {
    let prefix = format!("{key} = \"");
    manifest
        .lines()
        .find_map(|line| line.strip_prefix(&prefix)?.strip_suffix('"'))
        .ok_or_else(|| build_error(format!("libghostty manifest is missing `{key}`")))
}

fn info_value<'a>(info: &'a str, key: &str) -> BuildResult<&'a str> {
    let prefix = format!("{key}=");
    info.lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| build_error(format!("libghostty build info is missing `{key}`")))
}

fn artifact_info_value<'a>(
    info: &'a str,
    key: &str,
    build_info: &Path,
    target: &str,
    action: &str,
) -> BuildResult<&'a str> {
    info_value(info, key).map_err(|error| {
        artifact_error(
            target,
            build_info,
            format!("incoherent build metadata: {error}"),
            action,
        )
    })
}

fn verify_hash(path: &Path, expected: &str, target: &str, action: &str) -> BuildResult<()> {
    let bytes = fs::read(path).map_err(|error| {
        artifact_error(target, path, format!("cannot hash input: {error}"), action)
    })?;
    verify_digest(path, Sha256::digest(bytes), expected, target, action)
}

fn verify_text_hash(path: &Path, expected: &str, target: &str, action: &str) -> BuildResult<()> {
    let text = fs::read_to_string(path).map_err(|error| {
        artifact_error(
            target,
            path,
            format!("cannot read UTF-8 input: {error}"),
            action,
        )
    })?;
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    verify_digest(
        path,
        Sha256::digest(normalized.as_bytes()),
        expected,
        target,
        action,
    )
}

fn verify_digest(
    path: &Path,
    actual: impl std::fmt::LowerHex,
    expected: &str,
    target: &str,
    action: &str,
) -> BuildResult<()> {
    let actual = format!("{actual:x}");
    if actual == expected {
        Ok(())
    } else {
        Err(artifact_error(
            target,
            path,
            format!("checksum mismatch: expected {expected}, got {actual}"),
            action,
        ))
    }
}

fn artifact_error(
    target: &str,
    path: &Path,
    detail: impl std::fmt::Display,
    action: &str,
) -> Box<dyn Error> {
    build_error(format!(
        "libghostty input rejected for target {target}: {}: {detail}. Corrective action: {action}",
        path.display()
    ))
}

fn build_error(message: impl Into<String>) -> Box<dyn Error> {
    Box::new(io::Error::other(message.into()))
}
