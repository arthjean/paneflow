use sha2::{Digest, Sha256};
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

type BuildResult<T> = Result<T, Box<dyn Error>>;

fn main() -> BuildResult<()> {
    println!("cargo:rerun-if-env-changed=PANEFLOW_LIBGHOSTTY_DIR");
    let crate_dir = PathBuf::from(required_env("CARGO_MANIFEST_DIR")?);
    let workspace = crate_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| build_error("the -sys crate must live under <workspace>/crates"))?
        .to_path_buf();
    let manifest_path = workspace.join("native/libghostty/manifest.toml");
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    println!(
        "cargo:rerun-if-changed={}",
        workspace.join("native/libghostty/bindings.rs").display()
    );

    let manifest = fs::read_to_string(&manifest_path).map_err(|error| {
        build_error(format!("cannot read {}: {error}", manifest_path.display()))
    })?;
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
    if !target.contains("-linux-") {
        return Ok(());
    }

    verify_hash(
        &workspace.join(manifest_value(&manifest, "bindings_path")?),
        manifest_value(&manifest, "bindings_sha256")?,
    )?;

    let bundled = workspace.join("native/libghostty/prebuilt").join(&target);
    let (prepared, uses_bundled_archive) = match std::env::var_os("PANEFLOW_LIBGHOSTTY_DIR") {
        Some(path) => (PathBuf::from(path), false),
        None => (bundled, true),
    };
    let archive = prepared.join("lib/libghostty-vt.a");
    let header = prepared.join("include/ghostty/vt.h");
    let bindings = prepared.join("bindings.rs");
    let build_info = prepared.join("build-info.txt");

    for path in [&archive, &header, &bindings, &build_info] {
        if !path.is_file() {
            return Err(build_error(format!(
                "libghostty input for {target} is incomplete: missing {}. Restore native/libghostty/prebuilt, or run scripts/build-libghostty-linux.sh --target {target} and set PANEFLOW_LIBGHOSTTY_DIR to its output; Cargo performs no downloads",
                path.display()
            )));
        }
        println!("cargo:rerun-if-changed={}", path.display());
    }

    verify_hash(&header, manifest_value(&manifest, "header_sha256")?)?;
    verify_hash(&bindings, manifest_value(&manifest, "bindings_sha256")?)?;
    let info = fs::read_to_string(&build_info)
        .map_err(|error| build_error(format!("cannot read {}: {error}", build_info.display())))?;
    let (archive_hash_key, zig_target) = match target.as_str() {
        "x86_64-unknown-linux-gnu" => (
            "archive_sha256_x86_64_unknown_linux_gnu",
            "x86_64-linux-gnu",
        ),
        "aarch64-unknown-linux-gnu" => (
            "archive_sha256_aarch64_unknown_linux_gnu",
            "aarch64-linux-gnu",
        ),
        _ => {
            return Err(build_error(format!(
                "libghostty has no reviewed static archive for Linux target {target}"
            )));
        }
    };
    for (key, value) in [
        ("source_sha", manifest_value(&manifest, "source_sha")?),
        ("zig_version", manifest_value(&manifest, "zig_version")?),
        ("header_sha256", manifest_value(&manifest, "header_sha256")?),
        (
            "bindings_sha256",
            manifest_value(&manifest, "bindings_sha256")?,
        ),
        ("rust_target", target.as_str()),
        ("zig_target", zig_target),
        ("optimize", manifest_value(&manifest, "build_mode")?),
        (
            "archive_normalization",
            manifest_value(&manifest, "archive_normalization")?,
        ),
        (
            "build_info_symbol",
            manifest_value(&manifest, "build_info_symbol")?,
        ),
    ] {
        if info_value(&info, key)? != value {
            return Err(build_error(format!(
                "libghostty build info mismatch for `{key}` in {}: expected `{value}`",
                build_info.display()
            )));
        }
    }

    let expected_archive_hash = manifest_value(&manifest, archive_hash_key)?;
    let prepared_archive_hash = info_value(&info, "archive_sha256")?;
    if uses_bundled_archive && prepared_archive_hash != expected_archive_hash {
        return Err(build_error(format!(
            "libghostty build info archive checksum mismatch in {}: expected `{expected_archive_hash}`",
            build_info.display()
        )));
    }
    // Bundled archives are reviewed byte-for-byte in the manifest. An explicit
    // prepared directory is rebuilt from the pinned source and toolchain, so
    // verify its build-info hash instead; object normalization can differ
    // across host elfutils versions even when the rebuild is reproducible.
    let archive_hash = if uses_bundled_archive {
        expected_archive_hash
    } else {
        prepared_archive_hash
    };
    verify_hash(&archive, archive_hash)?;

    let link_dir = archive
        .parent()
        .ok_or_else(|| build_error("archive path must have a parent"))?;
    println!("cargo:rustc-link-search=native={}", link_dir.display());
    println!("cargo:rustc-link-lib=static=ghostty-vt");
    Ok(())
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

fn verify_hash(path: &Path, expected: &str) -> BuildResult<()> {
    let bytes = fs::read(path)
        .map_err(|error| build_error(format!("cannot hash {}: {error}", path.display())))?;
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual != expected {
        return Err(build_error(format!(
            "libghostty checksum mismatch at {}: expected {expected}, got {actual}",
            path.display()
        )));
    }
    Ok(())
}

fn build_error(message: impl Into<String>) -> Box<dyn Error> {
    Box::new(io::Error::other(message.into()))
}
