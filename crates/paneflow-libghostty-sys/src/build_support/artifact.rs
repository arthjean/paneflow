use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

use super::checksum::{validate_sha256, verify_hash, verify_text_hash};
use super::manifest::{NativePlatform, TargetContract, WindowsContract};
use super::{BuildResult, artifact_error, build_error};

pub(crate) struct ArtifactBundle {
    root: PathBuf,
    archive: PathBuf,
    header: PathBuf,
    bindings: PathBuf,
    build_info: PathBuf,
    headers_index: PathBuf,
    symbols: PathBuf,
    uses_bundled_archive: bool,
    platform: NativePlatform,
}

#[derive(Debug)]
struct BuildInfo {
    values: HashMap<String, String>,
}

impl ArtifactBundle {
    pub(crate) fn resolve(
        workspace: &Path,
        contract: &TargetContract,
        prepared_override: Option<OsString>,
    ) -> Self {
        let bundled = workspace
            .join("native/libghostty/prebuilt")
            .join(contract.target());
        let (root, uses_bundled_archive) = match prepared_override {
            Some(path) => (PathBuf::from(path), false),
            None => (bundled, true),
        };
        Self {
            archive: root.join(&contract.archive_path),
            header: root.join(contract.header_path()),
            bindings: root.join("bindings.rs"),
            build_info: root.join("build-info.txt"),
            headers_index: root.join("headers.sha256"),
            symbols: root.join("symbols.txt"),
            root,
            uses_bundled_archive,
            platform: contract.platform(),
        }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn required_inputs(&self) -> Vec<&Path> {
        let mut inputs = vec![
            self.archive.as_path(),
            self.header.as_path(),
            self.bindings.as_path(),
            self.build_info.as_path(),
        ];
        match self.platform {
            NativePlatform::Linux | NativePlatform::Macos => {}
            NativePlatform::Windows => {
                inputs.extend([self.headers_index.as_path(), self.symbols.as_path()]);
            }
        }
        inputs
    }

    pub(crate) fn requires_directory_watch(&self) -> bool {
        self.has_windows_inventory()
    }

    fn has_windows_inventory(&self) -> bool {
        match self.platform {
            NativePlatform::Linux | NativePlatform::Macos => false,
            NativePlatform::Windows => true,
        }
    }

    pub(crate) fn link_directory(&self) -> BuildResult<&Path> {
        self.archive
            .parent()
            .ok_or_else(|| build_error("archive path must have a parent"))
    }

    pub(crate) fn validate(&self, contract: &TargetContract, action: &str) -> BuildResult<()> {
        validate_directory(&self.root)
            .map_err(|detail| artifact_error(contract.target(), &self.root, detail, action))?;
        for path in self.required_inputs() {
            require_file_beneath(&self.root, path, contract.target(), action)?;
        }
        verify_artifact_text(
            &self.header,
            &contract.header_sha256,
            contract.target(),
            action,
        )?;
        verify_artifact_text(
            &self.bindings,
            &contract.bindings_sha256,
            contract.target(),
            action,
        )?;

        let info_source = fs::read_to_string(&self.build_info).map_err(|error| {
            artifact_error(
                contract.target(),
                &self.build_info,
                format!("cannot read build metadata: {error}"),
                action,
            )
        })?;
        let info = BuildInfo::parse(&info_source).map_err(|detail| {
            artifact_error(contract.target(), &self.build_info, detail, action)
        })?;
        for (key, expected) in contract.expected_build_info() {
            let actual = info.required(key).map_err(|detail| {
                artifact_error(contract.target(), &self.build_info, detail, action)
            })?;
            if actual != expected {
                return Err(artifact_error(
                    contract.target(),
                    &self.build_info,
                    format!("build metadata `{key}` is `{actual}`, expected `{expected}`"),
                    action,
                ));
            }
        }

        let prepared_archive_hash = info.required("archive_sha256").map_err(|detail| {
            artifact_error(contract.target(), &self.build_info, detail, action)
        })?;
        validate_sha256(prepared_archive_hash).map_err(|detail| {
            artifact_error(contract.target(), &self.build_info, detail, action)
        })?;
        if (self.uses_bundled_archive || self.has_windows_inventory())
            && prepared_archive_hash != contract.archive_sha256
        {
            return Err(artifact_error(
                contract.target(),
                &self.build_info,
                format!(
                    "archive checksum metadata is `{prepared_archive_hash}`, expected `{}`",
                    contract.archive_sha256
                ),
                action,
            ));
        }
        let archive_hash = if self.uses_bundled_archive || self.has_windows_inventory() {
            contract.archive_sha256.as_str()
        } else {
            prepared_archive_hash
        };
        verify_hash(&self.archive, archive_hash)
            .map_err(|detail| artifact_error(contract.target(), &self.archive, detail, action))?;

        if let Some(windows) = contract.windows() {
            self.validate_windows_metadata(contract, windows, &info, action)?;
        }
        Ok(())
    }

    fn validate_windows_metadata(
        &self,
        contract: &TargetContract,
        windows: &WindowsContract,
        info: &BuildInfo,
        action: &str,
    ) -> BuildResult<()> {
        for (path, expected, info_key) in [
            (
                &self.headers_index,
                windows.headers_index_sha256.as_str(),
                "headers_sha256",
            ),
            (
                &self.symbols,
                windows.symbols_sha256.as_str(),
                "symbols_sha256",
            ),
        ] {
            let recorded = info.required(info_key).map_err(|detail| {
                artifact_error(contract.target(), &self.build_info, detail, action)
            })?;
            if recorded != expected {
                return Err(artifact_error(
                    contract.target(),
                    path,
                    format!("build metadata hash is `{recorded}`, expected `{expected}`"),
                    action,
                ));
            }
            verify_artifact_text(path, expected, contract.target(), action)?;
        }

        verify_artifact_text(
            &self.build_info,
            &windows.build_info_sha256,
            contract.target(),
            action,
        )?;
        let indexed_headers = verify_header_index(
            &self.root.join("include"),
            &self.headers_index,
            contract.target(),
            action,
        )?;
        verify_windows_inventory(
            &self.root,
            &contract.archive_path,
            &indexed_headers,
            contract.target(),
            action,
        )?;

        let symbol_count = fs::read_to_string(&self.symbols)
            .map_err(|error| artifact_error(contract.target(), &self.symbols, error, action))?
            .lines()
            .filter(|line| !line.is_empty())
            .count()
            .to_string();
        if info
            .required("symbol_count")
            .map_err(|detail| artifact_error(contract.target(), &self.build_info, detail, action))?
            != symbol_count
        {
            return Err(artifact_error(
                contract.target(),
                &self.symbols,
                format!("symbol inventory count does not match build-info ({symbol_count})"),
                action,
            ));
        }
        Ok(())
    }
}

impl BuildInfo {
    fn parse(source: &str) -> Result<Self, String> {
        let mut values = HashMap::new();
        for (index, line) in source.lines().enumerate() {
            let line_number = index + 1;
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| format!("invalid build metadata at line {line_number}"))?;
            if key.is_empty() || value.is_empty() {
                return Err(format!(
                    "empty build metadata key or value at line {line_number}"
                ));
            }
            if values.insert(key.to_owned(), value.to_owned()).is_some() {
                return Err(format!("duplicate build metadata key `{key}`"));
            }
        }
        Ok(Self { values })
    }

    fn required(&self, key: &str) -> Result<&str, String> {
        self.values
            .get(key)
            .map(String::as_str)
            .ok_or_else(|| format!("libghostty build info is missing `{key}`"))
    }
}

pub(crate) fn validate_regular_file_beneath(root: &Path, path: &Path) -> Result<(), String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|error| format!("required input escaped its root: {error}"))?;
    let components = relative.components().collect::<Vec<_>>();
    if components.is_empty() {
        return Err("required input must be below its root".to_owned());
    }

    let mut current = root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        if !matches!(component, Component::Normal(_)) {
            return Err("required input contains an unsafe path component".to_owned());
        }
        current.push(component);
        let metadata = fs::symlink_metadata(&current)
            .map_err(|error| format!("cannot inspect required input: {error}"))?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "required input path contains symlink {}",
                current.display()
            ));
        }
        let is_last = index + 1 == components.len();
        if is_last && !metadata.is_file() {
            return Err("required input is not a regular file".to_owned());
        }
        if !is_last && !metadata.is_dir() {
            return Err(format!(
                "required input parent is not a directory: {}",
                current.display()
            ));
        }
    }
    Ok(())
}

fn require_file_beneath(root: &Path, path: &Path, target: &str, action: &str) -> BuildResult<()> {
    validate_regular_file_beneath(root, path)
        .map_err(|detail| artifact_error(target, path, detail, action))
}

fn validate_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect prepared root: {error}"))?;
    if metadata.file_type().is_symlink() {
        Err("prepared root must not be a symlink".to_owned())
    } else if metadata.is_dir() {
        Ok(())
    } else {
        Err("prepared root is not a directory".to_owned())
    }
}

fn verify_artifact_text(
    path: &Path,
    expected: &str,
    target: &str,
    action: &str,
) -> BuildResult<()> {
    verify_text_hash(path, expected).map_err(|detail| artifact_error(target, path, detail, action))
}

fn verify_header_index(
    include_root: &Path,
    index: &Path,
    target: &str,
    action: &str,
) -> BuildResult<HashSet<PathBuf>> {
    let contents =
        fs::read_to_string(index).map_err(|error| artifact_error(target, index, error, action))?;
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
        validate_sha256(expected)
            .map_err(|detail| artifact_error(target, index, detail, action))?;
        let relative_path = Path::new(relative);
        if relative_path.is_absolute()
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
        require_file_beneath(include_root, &header, target, action)?;
        verify_artifact_text(&header, expected, target, action)?;
    }
    Ok(indexed)
}

fn verify_windows_inventory(
    prepared_root: &Path,
    archive_path: &Path,
    indexed_headers: &HashSet<PathBuf>,
    target: &str,
    action: &str,
) -> BuildResult<()> {
    let mut expected = HashSet::from([
        PathBuf::from("bindings.rs"),
        PathBuf::from("build-info.txt"),
        PathBuf::from("headers.sha256"),
        PathBuf::from("symbols.txt"),
        archive_path.to_path_buf(),
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

#[cfg(test)]
mod tests {
    use super::super::manifest::Manifest;
    use super::super::manifest::tests::{MACOS_TARGET, macos_manifest};
    use super::*;
    use sha2::{Digest, Sha256};

    const MANIFEST: &str = include_str!("../../../../native/libghostty/manifest.toml");

    #[test]
    fn build_info_rejects_duplicate_keys() {
        let error = BuildInfo::parse("source_sha=one\nsource_sha=two\n")
            .expect_err("duplicate metadata must be rejected");
        assert!(error.contains("duplicate build metadata key"));
    }

    #[test]
    fn build_info_reports_missing_required_key() {
        let info = BuildInfo::parse("source_sha=one\n").expect("fixture must parse");
        let error = info
            .required("zig_version")
            .expect_err("missing metadata must be rejected");
        assert!(error.contains("missing `zig_version`"));
    }

    #[test]
    fn header_index_rejects_parent_traversal() -> BuildResult<()> {
        let root = tempfile::tempdir()?;
        let index = root.path().join("headers.sha256");
        let digest = format!("{:x}", Sha256::digest(b"header"));
        fs::write(&index, format!("{digest}  ../escape.h\n"))?;
        let error = verify_header_index(root.path(), &index, "test-target", "replace fixture")
            .expect_err("parent traversal must be rejected");
        assert!(error.to_string().contains("unsafe header index entry"));
        Ok(())
    }

    #[test]
    fn windows_inventory_rejects_missing_archive() -> BuildResult<()> {
        let root = tempfile::tempdir()?;
        for path in [
            "bindings.rs",
            "build-info.txt",
            "headers.sha256",
            "symbols.txt",
        ] {
            fs::write(root.path().join(path), [])?;
        }
        let error = verify_windows_inventory(
            root.path(),
            Path::new("lib/ghostty-vt-static.lib"),
            &HashSet::new(),
            "test-target",
            "replace fixture",
        )
        .expect_err("an incomplete inventory must be rejected");
        assert!(error.to_string().contains("lib/ghostty-vt-static.lib"));
        Ok(())
    }

    #[test]
    fn validates_the_reviewed_linux_bundle_as_one_unit() -> BuildResult<()> {
        validates_the_reviewed_bundle("x86_64-unknown-linux-gnu")
    }

    #[test]
    fn validates_the_reviewed_macos_bundle_as_one_unit() -> BuildResult<()> {
        validates_the_reviewed_bundle(MACOS_TARGET)
    }

    fn validates_the_reviewed_bundle(target: &str) -> BuildResult<()> {
        let manifest = Manifest::parse(MANIFEST)?;
        let contract = manifest.target_contract(target)?;
        let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace = crate_dir
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| build_error("test crate must live under <workspace>/crates"))?;
        let bundle = ArtifactBundle::resolve(workspace, &contract, None);
        bundle.validate(&contract, "replace reviewed fixture")
    }

    fn macos_fixture(root: &Path) -> BuildResult<(String, Vec<(String, String)>)> {
        const HEADER: &str = "#define GHOSTTY_VT 1\n";
        const BINDINGS: &str = "// generated bindings\n";
        const ARCHIVE: &[u8] = b"!<arch>\nfixture";
        const NORMALIZATION: &str = "fixed-zig-source-cache-prefix+zig-build-seed0-j1-cpu1+llvm-strip-debug+llvm-ar-D-darwin";

        fs::create_dir_all(root.join("include/ghostty"))?;
        fs::create_dir_all(root.join("lib"))?;
        fs::write(root.join("include/ghostty/vt.h"), HEADER)?;
        fs::write(root.join("bindings.rs"), BINDINGS)?;
        fs::write(root.join("lib/libghostty-vt.a"), ARCHIVE)?;

        let archive_sha256 = format!("{:x}", Sha256::digest(ARCHIVE));
        let header_sha256 = format!("{:x}", Sha256::digest(HEADER.as_bytes()));
        let bindings_sha256 = format!("{:x}", Sha256::digest(BINDINGS.as_bytes()));
        let source = macos_manifest(&archive_sha256, "[]");
        let source = replace_manifest_value(&source, "header_sha256", &header_sha256);
        let source = replace_manifest_value(&source, "bindings_sha256", &bindings_sha256);

        let source_sha = manifest_value(MANIFEST, "source_sha");
        let zig_version = manifest_value(MANIFEST, "zig_version");
        let build_info = [
            ("source_sha", source_sha.as_str()),
            ("zig_version", zig_version.as_str()),
            ("header_sha256", header_sha256.as_str()),
            ("bindings_sha256", bindings_sha256.as_str()),
            ("rust_target", MACOS_TARGET),
            ("zig_target", "aarch64-macos"),
            ("optimize", "ReleaseFast"),
            ("archive_normalization", NORMALIZATION),
            ("archive_sha256", archive_sha256.as_str()),
            ("build_info_symbol", "ghostty_build_info"),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect::<Vec<_>>();
        write_build_info(root, &build_info)?;
        Ok((source, build_info))
    }

    fn write_build_info(root: &Path, entries: &[(String, String)]) -> BuildResult<()> {
        let body = entries
            .iter()
            .map(|(key, value)| format!("{key}={value}\n"))
            .collect::<String>();
        fs::write(root.join("build-info.txt"), body)?;
        Ok(())
    }

    fn manifest_value(source: &str, key: &str) -> String {
        let prefix = format!("{key} = \"");
        source
            .lines()
            .find_map(|line| line.strip_prefix(&prefix))
            .and_then(|value| value.strip_suffix('"'))
            .expect("the reviewed manifest declares the requested pin key")
            .to_owned()
    }

    fn replace_manifest_value(source: &str, key: &str, value: &str) -> String {
        source
            .lines()
            .map(|line| {
                if line.starts_with(&format!("{key} = \"")) {
                    format!("{key} = \"{value}\"")
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn macos_bundle_requires_only_the_platform_neutral_inputs() -> BuildResult<()> {
        let root = tempfile::tempdir()?;
        let (source, _) = macos_fixture(root.path())?;
        let manifest = Manifest::parse(&source)?;
        let contract = manifest.target_contract(MACOS_TARGET)?;
        let bundle = ArtifactBundle::resolve(root.path(), &contract, Some(root.path().into()));
        let inputs = bundle
            .required_inputs()
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();
        assert_eq!(inputs.len(), 4, "unexpected macOS inventory: {inputs:?}");
        assert!(inputs.iter().any(|path| path.ends_with("libghostty-vt.a")));
        assert!(inputs.iter().any(|path| path.ends_with("vt.h")));
        assert!(inputs.iter().any(|path| path.ends_with("bindings.rs")));
        assert!(inputs.iter().any(|path| path.ends_with("build-info.txt")));
        assert!(!inputs.iter().any(|path| path.ends_with("headers.sha256")));
        assert!(!inputs.iter().any(|path| path.ends_with("symbols.txt")));
        assert!(!bundle.requires_directory_watch());
        bundle.validate(&contract, &contract.corrective_action())
    }

    #[test]
    fn macos_bundle_names_every_missing_build_info_key() -> BuildResult<()> {
        let root = tempfile::tempdir()?;
        let (source, build_info) = macos_fixture(root.path())?;
        let manifest = Manifest::parse(&source)?;
        let contract = manifest.target_contract(MACOS_TARGET)?;
        let action = contract.corrective_action();
        assert_eq!(build_info.len(), 10);
        for (key, _) in &build_info {
            let reduced = build_info
                .iter()
                .filter(|(candidate, _)| candidate != key)
                .cloned()
                .collect::<Vec<_>>();
            write_build_info(root.path(), &reduced)?;
            let bundle = ArtifactBundle::resolve(root.path(), &contract, Some(root.path().into()));
            let error = bundle
                .validate(&contract, &action)
                .expect_err("a truncated build-info.txt must be rejected");
            let error = error.to_string();
            assert!(
                error.contains(&format!("missing `{key}`")),
                "error must name the missing key `{key}`: {error}"
            );
            assert!(error.contains("scripts/build-libghostty-macos.sh"));
        }
        write_build_info(root.path(), &build_info)?;
        Ok(())
    }

    #[test]
    fn macos_bundled_tree_is_pinned_to_the_manifest_checksum() -> BuildResult<()> {
        let workspace = tempfile::tempdir()?;
        let root = workspace
            .path()
            .join("native/libghostty/prebuilt")
            .join(MACOS_TARGET);
        fs::create_dir_all(&root)?;
        let (source, _) = macos_fixture(&root)?;
        let manifest = Manifest::parse(&source)?;
        let contract = manifest.target_contract(MACOS_TARGET)?;
        let action = contract.corrective_action();
        ArtifactBundle::resolve(workspace.path(), &contract, None).validate(&contract, &action)?;

        fs::write(root.join("lib/libghostty-vt.a"), b"!<arch>\ntampered")?;
        let error = ArtifactBundle::resolve(workspace.path(), &contract, None)
            .validate(&contract, &action)
            .expect_err("a tampered bundled archive must be rejected")
            .to_string();
        assert!(error.contains("checksum"), "{error}");
        assert!(
            error.contains("scripts/build-libghostty-macos.sh"),
            "{error}"
        );
        Ok(())
    }

    #[test]
    fn macos_bundle_names_a_missing_required_file() -> BuildResult<()> {
        let root = tempfile::tempdir()?;
        let (source, _) = macos_fixture(root.path())?;
        let manifest = Manifest::parse(&source)?;
        let contract = manifest.target_contract(MACOS_TARGET)?;
        fs::remove_file(root.path().join("lib/libghostty-vt.a"))?;
        let bundle = ArtifactBundle::resolve(root.path(), &contract, Some(root.path().into()));
        let error = bundle
            .validate(&contract, &contract.corrective_action())
            .expect_err("a truncated macOS bundle must be rejected");
        let error = error.to_string();
        assert!(error.contains("libghostty-vt.a"), "{error}");
        assert!(
            error.contains("scripts/build-libghostty-macos.sh"),
            "{error}"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn macos_bundle_rejects_a_symlinked_required_input() -> BuildResult<()> {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir()?;
        let (source, _) = macos_fixture(root.path())?;
        let manifest = Manifest::parse(&source)?;
        let contract = manifest.target_contract(MACOS_TARGET)?;
        let bindings = root.path().join("bindings.rs");
        let elsewhere = root.path().join("bindings.real.rs");
        fs::rename(&bindings, &elsewhere)?;
        symlink(&elsewhere, &bindings)?;
        let bundle = ArtifactBundle::resolve(root.path(), &contract, Some(root.path().into()));
        let error = bundle
            .validate(&contract, &contract.corrective_action())
            .expect_err("symlinked required inputs must be rejected");
        assert!(error.to_string().contains("contains symlink"), "{error}");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_required_input() -> BuildResult<()> {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir()?;
        let target = root.path().join("target");
        let link = root.path().join("link");
        fs::write(&target, b"content")?;
        symlink(&target, &link)?;
        let error = validate_regular_file_beneath(root.path(), &link)
            .expect_err("symlinks must be rejected");
        assert!(error.contains("contains symlink"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_parent_directory() -> BuildResult<()> {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        fs::write(outside.path().join("archive.a"), b"content")?;
        let link = root.path().join("lib");
        symlink(outside.path(), &link)?;
        let archive = link.join("archive.a");
        let error = validate_regular_file_beneath(root.path(), &archive)
            .expect_err("symlinked parent directories must be rejected");
        assert!(error.contains("contains symlink"));
        Ok(())
    }
}
