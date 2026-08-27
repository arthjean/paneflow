mod artifact;
mod checksum;
mod manifest;

use artifact::ArtifactBundle;
use manifest::Manifest;
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub type BuildResult<T> = Result<T, Box<dyn Error>>;

pub fn run() -> BuildResult<()> {
    println!("cargo:rerun-if-env-changed=PANEFLOW_LIBGHOSTTY_DIR");
    emit_ghostty_native_cfg();

    let crate_dir = PathBuf::from(required_env("CARGO_MANIFEST_DIR")?);
    let workspace = crate_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| build_error("the -sys crate must live under <workspace>/crates"))?;
    let manifest_path = workspace.join("native/libghostty/manifest.toml");
    let source = fs::read_to_string(&manifest_path).map_err(|error| {
        build_error(format!(
            "cannot read libghostty manifest {}: {error}",
            manifest_path.display()
        ))
    })?;
    let manifest = Manifest::parse(&source)?;
    println!("cargo:rerun-if-changed={}", manifest_path.display());

    let canonical_bindings = workspace.join(manifest.bindings_path());
    println!("cargo:rerun-if-changed={}", canonical_bindings.display());
    verify_workspace_text(
        workspace,
        &canonical_bindings,
        manifest.bindings_sha256(),
        "canonical libghostty bindings",
    )?;
    println!(
        "cargo:rustc-env=PANEFLOW_GHOSTTY_BINDINGS_PATH={}",
        canonical_bindings.display()
    );
    println!(
        "cargo:rustc-env=PANEFLOW_GHOSTTY_API_VERSION={}",
        manifest.api_version()
    );
    println!(
        "cargo:rustc-env=PANEFLOW_GHOSTTY_APP_VERSION={}",
        manifest.ghostty_app_version()
    );

    if std::env::var_os("CARGO_FEATURE_LINK").is_none() {
        return Ok(());
    }

    let target = required_env("TARGET")?;
    let contract = manifest.target_contract(&target)?;
    let action = contract.corrective_action();

    let bundle = ArtifactBundle::resolve(
        workspace,
        &contract,
        std::env::var_os("PANEFLOW_LIBGHOSTTY_DIR"),
    );
    for path in bundle.required_inputs() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    if bundle.requires_directory_watch() {
        println!("cargo:rerun-if-changed={}", bundle.root().display());
    }
    bundle.validate(&contract, &action)?;

    println!(
        "cargo:rustc-link-search=native={}",
        bundle.link_directory()?.display()
    );
    println!("cargo:rustc-link-lib=static={}", contract.link_name());
    for library in contract.system_libraries() {
        println!("cargo:rustc-link-lib=dylib={library}");
    }
    Ok(())
}

/// Emit the `ghostty_native` cfg alias for this crate.
///
/// `ghostty_native` is the single spelling of "the native Ghostty FFI layer
/// is linked into this build". It replaces the platform-and-feature predicate
/// that used to be copy-pasted across every gate site, so adding a target is
/// a change to [`ghostty_native_target`] and the manifest rather than a
/// textual sweep over nine files.
///
/// The `rustc-check-cfg` directive is printed unconditionally so that a build
/// where the alias is *not* set still declares it as a known cfg name and the
/// `unexpected_cfgs` lint stays quiet.
///
/// The predicate is evaluated from the `CARGO_CFG_TARGET_*` and
/// `CARGO_FEATURE_*` variables Cargo sets for the *target* being built, never
/// from `cfg!()` on the build-script host, which would describe the host.
fn emit_ghostty_native_cfg() {
    println!("cargo::rustc-check-cfg=cfg(ghostty_native)");
    // The `-sys` crate only compiles and links the FFI layer under the `link`
    // feature; `paneflow-terminal-ghostty/native` is what turns it on.
    if std::env::var_os("CARGO_FEATURE_LINK").is_some() && ghostty_native_target() {
        println!("cargo::rustc-cfg=ghostty_native");
    }
}

/// Whether the target triple being built has a declared libghostty archive.
///
/// Kept in sync with the `[[target]]` entries of
/// `native/libghostty/manifest.toml`. A target absent from this list resolves
/// to the stub path with no `ghostty_native` cfg emitted.
fn ghostty_native_target() -> bool {
    let cfg = |key: &str| std::env::var(key).unwrap_or_default();
    match cfg("CARGO_CFG_TARGET_OS").as_str() {
        "linux" => true,
        // Only Apple Silicon has a declared archive; x86_64-apple-darwin is a
        // closed release target and resolves to the stub path.
        "macos" => cfg("CARGO_CFG_TARGET_ARCH") == "aarch64",
        "windows" => {
            cfg("CARGO_CFG_TARGET_ARCH") == "x86_64" && cfg("CARGO_CFG_TARGET_ENV") == "msvc"
        }
        _ => false,
    }
}

fn verify_workspace_text(
    workspace: &Path,
    path: &Path,
    expected: &str,
    label: &str,
) -> BuildResult<()> {
    artifact::validate_regular_file_beneath(workspace, path).map_err(|detail| {
        build_error(format!("{label} rejected at {}: {detail}", path.display()))
    })?;
    checksum::verify_text_hash(path, expected)
        .map_err(|detail| build_error(format!("{label} rejected at {}: {detail}", path.display())))
}

fn required_env(key: &str) -> BuildResult<String> {
    std::env::var(key).map_err(|_| build_error(format!("Cargo did not set {key}")))
}

pub(crate) fn artifact_error(
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

pub(crate) fn build_error(message: impl Into<String>) -> Box<dyn Error> {
    Box::new(io::Error::other(message.into()))
}
