#![allow(clippy::panic)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const EMBED_SIZE_LIMIT_BYTES: u64 = 1_835_008;
fn main() {
    println!("cargo:rerun-if-env-changed=POSTHOG_API_KEY");
    println!("cargo:rerun-if-env-changed=POSTHOG_HOST");
    println!("cargo:rerun-if-env-changed=PANEFLOW_SKIP_EMBED_BUILD");

    assert_ghostty_target_is_supported();

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-lib=framework=Metal");
    }

    let target = std::env::var("TARGET").expect("cargo always sets TARGET for build scripts");
    println!("cargo:rustc-env=PANEFLOW_TARGET_TRIPLE={target}");

    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR")
            .expect("cargo always sets CARGO_MANIFEST_DIR for build scripts"),
    );
    let workspace_root = manifest_dir
        .parent()
        .expect("src-app manifest dir has a parent (the workspace root)")
        .to_path_buf();

    #[cfg(windows)]
    embed_windows_app_icon(&workspace_root);

    let embed_root = manifest_dir.join("target").join("embed").join("bin");
    let embed_dir = embed_root.join(&target);
    fs::create_dir_all(&embed_dir).unwrap_or_else(|e| {
        panic!(
            "US-008: cannot create embed staging dir {}: {e}",
            embed_dir.display()
        )
    });

    println!(
        "cargo:rerun-if-changed={}",
        workspace_root.join("crates/paneflow-shim").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        workspace_root.join("crates/paneflow-ai-hook").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        workspace_root.join("crates/paneflow-mcp").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        workspace_root.join("Cargo.toml").display()
    );
    for asset in [
        "crates/paneflow-shim/assets/opencode-paneflow-status.ts",
        "crates/paneflow-shim/assets/pi-paneflow-status.ts",
    ] {
        println!(
            "cargo:rerun-if-changed={}",
            workspace_root.join(asset).display()
        );
    }

    let skip_nested_build = std::env::var_os("PANEFLOW_SKIP_EMBED_BUILD").is_some();
    if !skip_nested_build {
        stage_ai_hook_binaries(&workspace_root, &target, &embed_dir);
    } else {
        println!(
            "cargo:warning=PANEFLOW_SKIP_EMBED_BUILD is set - assuming {} is already populated",
            embed_dir.display()
        );
    }

    enforce_embed_size_budget(&embed_dir);
}

fn stage_ai_hook_binaries(workspace_root: &Path, target: &str, embed_dir: &Path) {
    let nested_target_dir = workspace_root.join("target").join("embed-build");

    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let profile = "release-min";

    let mut cmd = Command::new(&cargo);
    cmd.current_dir(workspace_root)
        .arg("build")
        .arg("--profile")
        .arg(profile)
        .arg("--target")
        .arg(target)
        .arg("--target-dir")
        .arg(&nested_target_dir)
        .arg("-p")
        .arg("paneflow-shim")
        .arg("-p")
        .arg("paneflow-ai-hook")
        .arg("-p")
        .arg("paneflow-mcp")
        .env_remove("CARGO_TARGET_DIR");

    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("US-008: failed to spawn nested cargo build: {e}"));
    if !status.success() {
        panic!(
            "US-008: nested `cargo build --profile {profile} -p paneflow-shim -p paneflow-ai-hook -p paneflow-mcp --target {target}` \
             failed with {status}. Re-run the outer build with verbose logging to see the child cargo output."
        );
    }

    let artifact_dir = nested_target_dir.join(target).join(profile);

    let bin_exe = if target.contains("windows") {
        ".exe"
    } else {
        ""
    };

    for bin in ["paneflow-shim", "paneflow-ai-hook", "paneflow-mcp"] {
        let src = artifact_dir.join(format!("{bin}{bin_exe}"));
        let dst = embed_dir.join(format!("{bin}{bin_exe}"));

        if !src.exists() {
            panic!(
                "US-008: expected nested build artifact {} is missing - \
                 did the child cargo build silently skip this binary?",
                src.display()
            );
        }
        fs::copy(&src, &dst).unwrap_or_else(|e| {
            panic!(
                "US-008: copy {} → {} failed: {e}",
                src.display(),
                dst.display()
            )
        });
    }
}

fn enforce_embed_size_budget(embed_dir: &Path) {
    let mut total: u64 = 0;
    let mut per_file: BTreeMap<String, u64> = BTreeMap::new();
    let iter = match fs::read_dir(embed_dir) {
        Ok(iter) => iter,
        Err(e) => panic!(
            "US-008: cannot read embed staging dir {}: {e}",
            embed_dir.display()
        ),
    };
    for entry in iter {
        let entry = entry
            .unwrap_or_else(|e| panic!("US-008: broken embed dir entry in {embed_dir:?}: {e}"));
        let metadata = entry
            .metadata()
            .unwrap_or_else(|e| panic!("US-008: cannot stat {}: {e}", entry.path().display()));
        if metadata.is_file() {
            let size = metadata.len();
            total = total.saturating_add(size);
            per_file.insert(entry.file_name().to_string_lossy().into_owned(), size);
        }
    }

    if total > EMBED_SIZE_LIMIT_BYTES {
        let mut details = String::new();
        for (name, size) in &per_file {
            details.push_str(&format!("  {name}: {size} bytes\n"));
        }
        panic!(
            "US-008/EP-001: embedded binaries exceed the {EMBED_SIZE_LIMIT_BYTES}-byte cap ({total} bytes).\n\
             Staging dir: {}\n\
             Per-file:\n{details}\
             Shrink shim/ai-hook/paneflow-mcp via smaller deps or a tighter release-min profile, \
             or raise EMBED_SIZE_LIMIT_BYTES with a fresh measurement note.",
            embed_dir.display()
        );
    }
}

fn assert_ghostty_target_is_supported() {
    let cfg = |key: &str| std::env::var(key).unwrap_or_default();
    let arch = cfg("CARGO_CFG_TARGET_ARCH");
    let supported = match cfg("CARGO_CFG_TARGET_OS").as_str() {
        "linux" => arch == "x86_64" || arch == "aarch64",
        "macos" => arch == "aarch64",
        "windows" => arch == "x86_64" && cfg("CARGO_CFG_TARGET_ENV") == "msvc",
        _ => false,
    };
    assert!(
        supported,
        "paneflow-app has no libghostty archive for target {}. Ghostty is the only terminal \
         backend, so this target cannot be built. Declare it in \
         native/libghostty/manifest.toml and produce its archive with the matching \
         scripts/build-libghostty-* recipe first.",
        std::env::var("TARGET").unwrap_or_else(|_| "<unknown>".to_owned())
    );
}

#[cfg(windows)]
fn embed_windows_app_icon(workspace_root: &Path) {
    let icon = workspace_root.join("assets").join("PaneFlow.ico");
    let Some(icon_str) = icon.to_str() else {
        println!(
            "cargo:warning=winresource: icon path {} is not valid UTF-8; skipping exe icon embed",
            icon.display()
        );
        return;
    };
    if !icon.exists() {
        println!("cargo:warning=winresource: {icon_str} not found; skipping exe icon embed");
        return;
    }
    println!("cargo:rerun-if-changed={icon_str}");
    let mut res = winresource::WindowsResource::new();
    res.set_icon(icon_str);
    if let Err(e) = res.compile() {
        println!(
            "cargo:warning=winresource: failed to embed {icon_str} into paneflow.exe ({e}); \
             the exe will use the default Windows icon"
        );
    }
}
