use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use super::checksum::validate_sha256;
use super::{BuildResult, build_error};

const SUPPORTED_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Deserialize)]
pub(crate) struct Manifest {
    schema_version: u32,
    source_sha: String,
    ghostty_app_version: String,
    zig_version: String,
    header_path: PathBuf,
    header_sha256: String,
    bindings_path: PathBuf,
    bindings_sha256: String,
    api_version: String,
    build_mode: String,
    windows_zig_archive_url: Option<String>,
    windows_zig_archive_sha256: Option<String>,
    windows_zig_executable_sha256: Option<String>,
    windows_zig_image_base: Option<String>,
    windows_zig_dll_characteristics: Option<String>,
    windows_source_patch_path: Option<PathBuf>,
    windows_source_patch_sha256: Option<String>,
    windows_source_patch_target: Option<String>,
    windows_source_patch_input_sha256: Option<String>,
    windows_source_patch_output_sha256: Option<String>,
    windows_source_date_epoch: Option<String>,
    windows_canonical_source_path: Option<String>,
    windows_build_seed: Option<String>,
    windows_build_jobs: Option<String>,
    windows_msvc_toolset: Option<String>,
    windows_sdk: Option<String>,
    windows_llvm_version: Option<String>,
    windows_crt: Option<String>,
    windows_cxx_runtime: Option<String>,
    targets: BTreeMap<String, TargetConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "platform", rename_all = "lowercase")]
enum TargetConfig {
    Linux {
        archive_path: PathBuf,
        archive_sha256: String,
        archive_normalization: String,
        zig_target: String,
        link_name: String,
        system_libraries: Vec<String>,
        build_info_symbol: String,
    },
    Macos {
        archive_path: PathBuf,
        archive_sha256: String,
        archive_normalization: String,
        zig_target: String,
        link_name: String,
        system_libraries: Vec<String>,
        build_info_symbol: String,
    },
    Windows {
        archive_path: PathBuf,
        archive_sha256: String,
        archive_normalization: String,
        zig_target: String,
        link_name: String,
        system_libraries: Vec<String>,
        simd: bool,
        build_info_sha256: String,
        headers_index_sha256: String,
        symbols_sha256: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativePlatform {
    Linux,
    Macos,
    Windows,
}

#[derive(Debug)]
pub(crate) enum PlatformContract {
    Linux { build_info_symbol: String },
    Macos { build_info_symbol: String },
    Windows(Box<WindowsContract>),
}

#[derive(Debug)]
pub(crate) struct WindowsContract {
    pub(crate) source_patch_path: PathBuf,
    pub(crate) source_patch_sha256: String,
    pub(crate) build_info_sha256: String,
    pub(crate) headers_index_sha256: String,
    pub(crate) symbols_sha256: String,
    source_date_epoch: String,
    zig_archive_url: String,
    zig_archive_sha256: String,
    zig_executable_sha256: String,
    source_patch_target: String,
    source_patch_input_sha256: String,
    source_patch_output_sha256: String,
    zig_image_base: String,
    zig_dll_characteristics: String,
    simd: bool,
    build_seed: String,
    build_jobs: String,
    canonical_source_path: String,
    msvc_toolset: String,
    windows_sdk: String,
    llvm_version: String,
    crt: String,
    cxx_runtime: String,
}

#[derive(Debug)]
pub(crate) struct TargetContract {
    target: String,
    platform: NativePlatform,
    pub(crate) archive_path: PathBuf,
    pub(crate) archive_sha256: String,
    archive_normalization: String,
    zig_target: String,
    link_name: String,
    system_libraries: Vec<String>,
    source_sha: String,
    zig_version: String,
    header_path: PathBuf,
    pub(crate) header_sha256: String,
    pub(crate) bindings_sha256: String,
    build_mode: String,
    platform_contract: PlatformContract,
}

impl Manifest {
    pub(crate) fn parse(source: &str) -> BuildResult<Self> {
        let manifest: Self = toml::from_str(source)
            .map_err(|error| build_error(format!("invalid libghostty manifest: {error}")))?;
        if manifest.schema_version != SUPPORTED_SCHEMA_VERSION {
            return Err(build_error(format!(
                "unsupported libghostty manifest schema {}; expected {SUPPORTED_SCHEMA_VERSION}",
                manifest.schema_version
            )));
        }
        validate_safe_relative_path(&manifest.header_path, "header_path")?;
        validate_safe_relative_path(&manifest.bindings_path, "bindings_path")?;
        validate_manifest_digest(&manifest.header_sha256, "header_sha256")?;
        validate_manifest_digest(&manifest.bindings_sha256, "bindings_sha256")?;
        if manifest.targets.is_empty() {
            return Err(build_error("libghostty manifest declares no targets"));
        }
        for (target, config) in &manifest.targets {
            validate_target_config(target, config)?;
        }
        Ok(manifest)
    }

    pub(crate) fn api_version(&self) -> &str {
        &self.api_version
    }

    pub(crate) fn ghostty_app_version(&self) -> &str {
        &self.ghostty_app_version
    }

    pub(crate) fn bindings_path(&self) -> &Path {
        &self.bindings_path
    }

    pub(crate) fn bindings_sha256(&self) -> &str {
        &self.bindings_sha256
    }

    pub(crate) fn target_contract(&self, target: &str) -> BuildResult<TargetContract> {
        let config = self.targets.get(target).ok_or_else(|| {
            build_error(format!(
                "libghostty linking is unsupported for target {target}; reviewed targets: {}",
                self.targets.keys().cloned().collect::<Vec<_>>().join(", ")
            ))
        })?;

        let (
            platform,
            archive_path,
            archive_sha256,
            archive_normalization,
            zig_target,
            link_name,
            system_libraries,
            platform_contract,
        ) = match config {
            TargetConfig::Linux {
                archive_path,
                archive_sha256,
                archive_normalization,
                zig_target,
                link_name,
                system_libraries,
                build_info_symbol,
            } => (
                NativePlatform::Linux,
                archive_path.clone(),
                archive_sha256.clone(),
                archive_normalization.clone(),
                zig_target.clone(),
                link_name.clone(),
                system_libraries.clone(),
                PlatformContract::Linux {
                    build_info_symbol: build_info_symbol.clone(),
                },
            ),
            TargetConfig::Macos {
                archive_path,
                archive_sha256,
                archive_normalization,
                zig_target,
                link_name,
                system_libraries,
                build_info_symbol,
            } => (
                NativePlatform::Macos,
                archive_path.clone(),
                archive_sha256.clone(),
                archive_normalization.clone(),
                zig_target.clone(),
                link_name.clone(),
                system_libraries.clone(),
                PlatformContract::Macos {
                    build_info_symbol: build_info_symbol.clone(),
                },
            ),
            TargetConfig::Windows {
                archive_path,
                archive_sha256,
                archive_normalization,
                zig_target,
                link_name,
                system_libraries,
                simd,
                build_info_sha256,
                headers_index_sha256,
                symbols_sha256,
            } => (
                NativePlatform::Windows,
                archive_path.clone(),
                archive_sha256.clone(),
                archive_normalization.clone(),
                zig_target.clone(),
                link_name.clone(),
                system_libraries.clone(),
                PlatformContract::Windows(Box::new(self.windows_contract(
                    *simd,
                    build_info_sha256,
                    headers_index_sha256,
                    symbols_sha256,
                )?)),
            ),
        };

        Ok(TargetContract {
            target: target.to_owned(),
            platform,
            archive_path,
            archive_sha256,
            archive_normalization,
            zig_target,
            link_name,
            system_libraries,
            source_sha: self.source_sha.clone(),
            zig_version: self.zig_version.clone(),
            header_path: self.header_path.clone(),
            header_sha256: self.header_sha256.clone(),
            bindings_sha256: self.bindings_sha256.clone(),
            build_mode: self.build_mode.clone(),
            platform_contract,
        })
    }

    fn windows_contract(
        &self,
        simd: bool,
        build_info_sha256: &str,
        headers_index_sha256: &str,
        symbols_sha256: &str,
    ) -> BuildResult<WindowsContract> {
        Ok(WindowsContract {
            source_patch_path: required_path(
                &self.windows_source_patch_path,
                "windows_source_patch_path",
            )?,
            source_patch_sha256: required_digest(
                self.windows_source_patch_sha256.as_deref(),
                "windows_source_patch_sha256",
            )?,
            build_info_sha256: validated_digest(build_info_sha256, "build_info_sha256")?,
            headers_index_sha256: validated_digest(headers_index_sha256, "headers_index_sha256")?,
            symbols_sha256: validated_digest(symbols_sha256, "symbols_sha256")?,
            source_date_epoch: required_string(
                self.windows_source_date_epoch.as_deref(),
                "windows_source_date_epoch",
            )?,
            zig_archive_url: required_string(
                self.windows_zig_archive_url.as_deref(),
                "windows_zig_archive_url",
            )?,
            zig_archive_sha256: required_digest(
                self.windows_zig_archive_sha256.as_deref(),
                "windows_zig_archive_sha256",
            )?,
            zig_executable_sha256: required_digest(
                self.windows_zig_executable_sha256.as_deref(),
                "windows_zig_executable_sha256",
            )?,
            source_patch_target: required_string(
                self.windows_source_patch_target.as_deref(),
                "windows_source_patch_target",
            )?,
            source_patch_input_sha256: required_digest(
                self.windows_source_patch_input_sha256.as_deref(),
                "windows_source_patch_input_sha256",
            )?,
            source_patch_output_sha256: required_digest(
                self.windows_source_patch_output_sha256.as_deref(),
                "windows_source_patch_output_sha256",
            )?,
            zig_image_base: required_string(
                self.windows_zig_image_base.as_deref(),
                "windows_zig_image_base",
            )?,
            zig_dll_characteristics: required_string(
                self.windows_zig_dll_characteristics.as_deref(),
                "windows_zig_dll_characteristics",
            )?,
            simd,
            build_seed: required_string(self.windows_build_seed.as_deref(), "windows_build_seed")?,
            build_jobs: required_string(self.windows_build_jobs.as_deref(), "windows_build_jobs")?,
            canonical_source_path: required_string(
                self.windows_canonical_source_path.as_deref(),
                "windows_canonical_source_path",
            )?,
            msvc_toolset: required_string(
                self.windows_msvc_toolset.as_deref(),
                "windows_msvc_toolset",
            )?,
            windows_sdk: required_string(self.windows_sdk.as_deref(), "windows_sdk")?,
            llvm_version: required_string(
                self.windows_llvm_version.as_deref(),
                "windows_llvm_version",
            )?,
            crt: required_string(self.windows_crt.as_deref(), "windows_crt")?,
            cxx_runtime: required_string(
                self.windows_cxx_runtime.as_deref(),
                "windows_cxx_runtime",
            )?,
        })
    }
}

impl TargetContract {
    pub(crate) fn target(&self) -> &str {
        &self.target
    }

    pub(crate) fn header_path(&self) -> &Path {
        &self.header_path
    }

    pub(crate) fn link_name(&self) -> &str {
        &self.link_name
    }

    pub(crate) fn system_libraries(&self) -> &[String] {
        &self.system_libraries
    }

    pub(crate) fn platform(&self) -> NativePlatform {
        self.platform
    }

    pub(crate) fn windows(&self) -> Option<&WindowsContract> {
        match &self.platform_contract {
            PlatformContract::Windows(contract) => Some(contract),
            PlatformContract::Linux { .. } | PlatformContract::Macos { .. } => None,
        }
    }

    pub(crate) fn source_patch(&self) -> Option<(&Path, &str)> {
        self.windows().map(|contract| {
            (
                contract.source_patch_path.as_path(),
                contract.source_patch_sha256.as_str(),
            )
        })
    }

    pub(crate) fn corrective_action(&self) -> String {
        match self.platform {
            NativePlatform::Linux => format!(
                "restore native/libghostty/prebuilt/{}, or run scripts/build-libghostty-linux.sh --target {} and set PANEFLOW_LIBGHOSTTY_DIR to its output; Cargo performs no downloads",
                self.target, self.target
            ),
            NativePlatform::Macos => format!(
                "restore native/libghostty/prebuilt/{}, or run scripts/build-libghostty-macos.sh --target {} and set PANEFLOW_LIBGHOSTTY_DIR to its output; Cargo performs no downloads",
                self.target, self.target
            ),
            NativePlatform::Windows => format!(
                "restore native/libghostty/prebuilt/{}, or run scripts/build-libghostty-windows.ps1 -VerifyReproducible; Cargo performs no downloads",
                self.target
            ),
        }
    }

    pub(crate) fn expected_build_info(&self) -> Vec<(&'static str, String)> {
        let mut expected = vec![
            ("source_sha", self.source_sha.clone()),
            ("zig_version", self.zig_version.clone()),
            ("header_sha256", self.header_sha256.clone()),
            ("bindings_sha256", self.bindings_sha256.clone()),
            ("rust_target", self.target.clone()),
            ("zig_target", self.zig_target.clone()),
            ("optimize", self.build_mode.clone()),
            ("archive_normalization", self.archive_normalization.clone()),
        ];
        match &self.platform_contract {
            PlatformContract::Linux { build_info_symbol }
            | PlatformContract::Macos { build_info_symbol } => {
                expected.push(("build_info_symbol", build_info_symbol.clone()));
            }
            PlatformContract::Windows(contract) => {
                let canonical_cache =
                    format!("{}/.paneflow-zig-cache", contract.canonical_source_path);
                let canonical_prefix =
                    format!("{}/.paneflow-zig-output", contract.canonical_source_path);
                let system_libraries = self
                    .system_libraries
                    .iter()
                    .map(|library| format!("{library}.lib"))
                    .collect::<Vec<_>>()
                    .join(",");
                expected.extend([
                    ("source_date_epoch", contract.source_date_epoch.clone()),
                    ("zig_archive_url", contract.zig_archive_url.clone()),
                    ("zig_archive_sha256", contract.zig_archive_sha256.clone()),
                    (
                        "zig_executable_sha256",
                        contract.zig_executable_sha256.clone(),
                    ),
                    (
                        "source_patch_path",
                        contract.source_patch_path.display().to_string(),
                    ),
                    ("source_patch_sha256", contract.source_patch_sha256.clone()),
                    ("source_patch_target", contract.source_patch_target.clone()),
                    (
                        "source_patch_input_sha256",
                        contract.source_patch_input_sha256.clone(),
                    ),
                    (
                        "source_patch_output_sha256",
                        contract.source_patch_output_sha256.clone(),
                    ),
                    ("zig_image_base", contract.zig_image_base.clone()),
                    (
                        "zig_dll_characteristics",
                        contract.zig_dll_characteristics.clone(),
                    ),
                    ("simd", contract.simd.to_string()),
                    ("build_seed", contract.build_seed.clone()),
                    ("build_jobs", contract.build_jobs.clone()),
                    (
                        "canonical_source_path",
                        contract.canonical_source_path.clone(),
                    ),
                    ("canonical_cache_path", canonical_cache),
                    ("canonical_prefix_path", canonical_prefix),
                    ("msvc_toolset", contract.msvc_toolset.clone()),
                    ("windows_sdk", contract.windows_sdk.clone()),
                    ("llvm_version", contract.llvm_version.clone()),
                    ("crt", contract.crt.clone()),
                    ("cxx_runtime", contract.cxx_runtime.clone()),
                    ("system_libraries", system_libraries),
                ]);
            }
        }
        expected
    }
}

fn validate_target_config(target: &str, config: &TargetConfig) -> BuildResult<()> {
    if target.is_empty() {
        return Err(build_error(
            "libghostty manifest contains an empty target triple",
        ));
    }
    let (archive_path, archive_sha256, zig_target, link_name, system_libraries) = match config {
        TargetConfig::Linux {
            archive_path,
            archive_sha256,
            zig_target,
            link_name,
            system_libraries,
            build_info_symbol,
            ..
        } => {
            if !system_libraries.is_empty() {
                return Err(build_error(format!(
                    "Linux libghostty target `{target}` must not declare system libraries"
                )));
            }
            validate_token(build_info_symbol, "build_info_symbol")?;
            (
                archive_path,
                archive_sha256,
                zig_target,
                link_name,
                system_libraries,
            )
        }
        TargetConfig::Macos {
            archive_path,
            archive_sha256,
            zig_target,
            link_name,
            system_libraries,
            build_info_symbol,
            ..
        } => {
            if !system_libraries.is_empty() {
                return Err(build_error(format!(
                    "macOS libghostty target `{target}` must not declare system libraries: libghostty-vt requires none on macOS"
                )));
            }
            validate_token(build_info_symbol, "build_info_symbol")?;
            (
                archive_path,
                archive_sha256,
                zig_target,
                link_name,
                system_libraries,
            )
        }
        TargetConfig::Windows {
            archive_path,
            archive_sha256,
            zig_target,
            link_name,
            system_libraries,
            build_info_sha256,
            headers_index_sha256,
            symbols_sha256,
            ..
        } => {
            validate_system_libraries(system_libraries)?;
            validate_manifest_digest(build_info_sha256, "build_info_sha256")?;
            validate_manifest_digest(headers_index_sha256, "headers_index_sha256")?;
            validate_manifest_digest(symbols_sha256, "symbols_sha256")?;
            (
                archive_path,
                archive_sha256,
                zig_target,
                link_name,
                system_libraries,
            )
        }
    };
    validate_safe_relative_path(archive_path, "archive_path")?;
    validate_manifest_digest(archive_sha256, "archive_sha256")?;
    validate_token(zig_target, "zig_target")?;
    validate_token(link_name, "link_name")?;
    for library in system_libraries {
        validate_token(library, "system_libraries")?;
    }
    Ok(())
}

fn validate_system_libraries(libraries: &[String]) -> BuildResult<()> {
    if libraries.is_empty() {
        return Err(build_error(
            "libghostty Windows contract has no system libraries",
        ));
    }
    Ok(())
}

fn validate_token(value: &str, key: &str) -> BuildResult<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(build_error(format!(
            "unsafe libghostty manifest `{key}` value `{value}`"
        )));
    }
    Ok(())
}

fn required_path(value: &Option<PathBuf>, key: &str) -> BuildResult<PathBuf> {
    let path = value
        .clone()
        .ok_or_else(|| build_error(format!("libghostty manifest is missing `{key}`")))?;
    validate_safe_relative_path(&path, key)?;
    Ok(path)
}

fn required_string(value: Option<&str>, key: &str) -> BuildResult<String> {
    value
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| build_error(format!("libghostty manifest is missing `{key}`")))
}

fn required_digest(value: Option<&str>, key: &str) -> BuildResult<String> {
    let value = required_string(value, key)?;
    validated_digest(&value, key)
}

fn validated_digest(value: &str, key: &str) -> BuildResult<String> {
    validate_manifest_digest(value, key)?;
    Ok(value.to_owned())
}

fn validate_manifest_digest(value: &str, key: &str) -> BuildResult<()> {
    validate_sha256(value)
        .map_err(|detail| build_error(format!("invalid libghostty manifest `{key}`: {detail}")))
}

fn validate_safe_relative_path(path: &Path, key: &str) -> BuildResult<()> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(build_error(format!(
            "libghostty manifest `{key}` must be a safe relative path"
        )));
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    const MANIFEST: &str = include_str!("../../../../native/libghostty/manifest.toml");
    pub(crate) const MACOS_TARGET: &str = "aarch64-apple-darwin";

    /// The reviewed manifest with the macOS target's archive digest and system
    /// library list overridden.
    ///
    /// `native/libghostty/manifest.toml` declares the target since US-007, so
    /// the fixture rewrites that block in place rather than appending a second
    /// one, which TOML would reject as a duplicate table. Every other field
    /// stays exactly as reviewed, and the contract is still exercised from the
    /// same `Manifest::parse` entry point the build script uses.
    pub(crate) fn macos_manifest(archive_sha256: &str, system_libraries: &str) -> String {
        let header = format!("[targets.\"{MACOS_TARGET}\"]");
        let start = MANIFEST
            .find(&header)
            .expect("the reviewed manifest declares the macOS target");
        let end = MANIFEST[start + header.len()..]
            .find("\n[")
            .map_or(MANIFEST.len(), |offset| start + header.len() + offset + 1);
        let patched: String = MANIFEST[start..end]
            .split_inclusive('\n')
            .map(|line| {
                if line.starts_with("archive_sha256 = ") {
                    format!("archive_sha256 = \"{archive_sha256}\"\n")
                } else if line.starts_with("system_libraries = ") {
                    format!("system_libraries = {system_libraries}\n")
                } else {
                    line.to_owned()
                }
            })
            .collect();
        format!("{}{patched}{}", &MANIFEST[..start], &MANIFEST[end..])
    }

    #[test]
    fn current_manifest_builds_typed_target_contracts() -> BuildResult<()> {
        let manifest = Manifest::parse(MANIFEST)?;
        let linux = manifest.target_contract("aarch64-unknown-linux-gnu")?;
        assert_eq!(linux.zig_target, "aarch64-linux-gnu");
        assert_eq!(linux.link_name(), "ghostty-vt");
        assert!(linux.system_libraries().is_empty());

        let macos = manifest.target_contract("aarch64-apple-darwin")?;
        assert_eq!(macos.platform(), NativePlatform::Macos);
        assert_eq!(macos.zig_target, "aarch64-macos");
        assert_eq!(macos.link_name(), "ghostty-vt");
        assert!(macos.system_libraries().is_empty());

        let windows = manifest.target_contract("x86_64-pc-windows-msvc")?;
        assert_eq!(windows.zig_target, "x86_64-windows-msvc");
        assert_eq!(windows.link_name(), "ghostty-vt-static");
        assert_eq!(windows.system_libraries(), ["ntdll", "kernel32"]);
        assert!(
            windows
                .expected_build_info()
                .contains(&("system_libraries", "ntdll.lib,kernel32.lib".to_owned()))
        );
        assert!(
            windows
                .expected_build_info()
                .contains(&("simd", "true".to_owned()))
        );
        Ok(())
    }

    #[test]
    fn rejects_unknown_schema_before_selecting_a_target() {
        let source = MANIFEST.replacen("schema_version = 2", "schema_version = 3", 1);
        let error = Manifest::parse(&source).expect_err("schema 3 must be rejected");
        assert!(
            error
                .to_string()
                .contains("unsupported libghostty manifest schema")
        );
    }

    #[test]
    fn rejects_undeclared_link_target() -> BuildResult<()> {
        let manifest = Manifest::parse(MANIFEST)?;
        // `aarch64-apple-darwin` is declared, so the undeclared-target
        // assertion uses the Intel Mac triple, which stays closed until a
        // separate PRD opens that target.
        let error = manifest
            .target_contract("x86_64-apple-darwin")
            .expect_err("undeclared targets must not silently skip linking");
        assert!(error.to_string().contains("linking is unsupported"));
        Ok(())
    }

    #[test]
    fn builds_the_macos_target_contract() -> BuildResult<()> {
        let manifest = Manifest::parse(&macos_manifest(&"0".repeat(64), "[]"))?;
        let macos = manifest.target_contract(MACOS_TARGET)?;
        assert_eq!(macos.platform(), NativePlatform::Macos);
        assert_eq!(macos.zig_target, "aarch64-macos");
        assert_eq!(macos.link_name(), "ghostty-vt");
        // `build_support::run` emits one `dylib` directive per system library,
        // so an empty list is what keeps the macOS link line to a single
        // `rustc-link-lib=static=ghostty-vt` with no `dylib` and no
        // `framework` entry.
        assert!(macos.system_libraries().is_empty());
        assert!(
            macos
                .corrective_action()
                .contains("scripts/build-libghostty-macos.sh")
        );
        Ok(())
    }

    #[test]
    fn macos_contract_requires_the_ten_keys_the_build_script_writes() -> BuildResult<()> {
        let manifest = Manifest::parse(&macos_manifest(&"0".repeat(64), "[]"))?;
        let macos = manifest.target_contract(MACOS_TARGET)?;
        let mut required = macos
            .expected_build_info()
            .into_iter()
            .map(|(key, _)| key)
            .collect::<Vec<_>>();
        // `archive_sha256` is required by `ArtifactBundle::validate` rather
        // than compared against a manifest string, so it completes the set
        // that `scripts/build-libghostty-macos.sh` writes.
        required.push("archive_sha256");
        required.sort_unstable();
        assert_eq!(
            required,
            [
                "archive_normalization",
                "archive_sha256",
                "bindings_sha256",
                "build_info_symbol",
                "header_sha256",
                "optimize",
                "rust_target",
                "source_sha",
                "zig_target",
                "zig_version",
            ]
        );
        assert!(
            macos
                .expected_build_info()
                .contains(&("rust_target", MACOS_TARGET.to_owned()))
        );
        assert!(
            macos
                .expected_build_info()
                .contains(&("build_info_symbol", "ghostty_build_info".to_owned()))
        );
        Ok(())
    }

    #[test]
    fn rejects_macos_system_libraries() {
        let source = macos_manifest(&"0".repeat(64), "[\"System\"]");
        let error = Manifest::parse(&source)
            .expect_err("libghostty-vt requires no system libraries on macOS");
        assert!(
            error
                .to_string()
                .contains("must not declare system libraries"),
            "unexpected error: {error}"
        );
        assert!(error.to_string().contains("requires none on macOS"));
    }

    #[test]
    fn rejects_an_unrecognized_platform_tag() {
        let source = macos_manifest(&"0".repeat(64), "[]")
            .replace("platform = \"macos\"", "platform = \"darwin\"");
        let error = Manifest::parse(&source)
            .expect_err("an unknown platform must be rejected")
            .to_string();
        assert!(
            error.contains("unknown variant `darwin`"),
            "the error must name the unrecognized platform: {error}"
        );
        assert!(
            error.contains("linux") && error.contains("macos") && error.contains("windows"),
            "the error must list the recognized platforms: {error}"
        );
    }

    #[test]
    fn rejects_workspace_path_escape() {
        let source = MANIFEST.replacen(
            "bindings_path = \"native/libghostty/bindings.rs\"",
            "bindings_path = \"../bindings.rs\"",
            1,
        );
        let error = Manifest::parse(&source).expect_err("parent traversal must be rejected");
        assert!(error.to_string().contains("safe relative path"));
    }
}
