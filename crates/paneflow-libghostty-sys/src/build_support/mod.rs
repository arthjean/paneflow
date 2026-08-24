mod artifact;
mod checksum;
mod manifest;

use artifact::ArtifactBundle;
use manifest::{Manifest, TargetContract};
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub type BuildResult<T> = Result<T, Box<dyn Error>>;

pub fn run() -> BuildResult<()> {
    println!("cargo:rerun-if-env-changed=PANEFLOW_LIBGHOSTTY_DIR");

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
    verify_platform_workspace_inputs(workspace, &contract, &action)?;

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

fn verify_platform_workspace_inputs(
    workspace: &Path,
    contract: &TargetContract,
    action: &str,
) -> BuildResult<()> {
    let Some((relative, expected_hash)) = contract.source_patch() else {
        return Ok(());
    };
    let path = workspace.join(relative);
    artifact::validate_regular_file_beneath(workspace, &path)
        .map_err(|detail| artifact_error(contract.target(), &path, detail, action))?;
    checksum::verify_text_hash(&path, expected_hash)
        .map_err(|detail| artifact_error(contract.target(), &path, detail, action))?;
    println!("cargo:rerun-if-changed={}", path.display());
    Ok(())
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
