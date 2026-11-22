use std::path::{Path, PathBuf};

const SHIPPING_TARGETS: &[&str] = &[
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
];

const RUNTIME_DEFINES: &[(&str, &str)] = &[
    ("WASM_RT_USE_MMAP", "0"),
    ("WASM_RT_MEMCHECK_BOUNDS_CHECK", "1"),
    ("WASM_RT_SKIP_SIGNAL_RECOVERY", "1"),
];

fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    assert!(
        SHIPPING_TARGETS.contains(&target.as_str()),
        "paneflow-highlights does not build for target {target}: it compiles only for the \
         shipping targets {}. Qualify the wasm2c lexers on a new target before adding it here.",
        SHIPPING_TARGETS.join(", ")
    );

    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default()).join("c");
    println!("cargo:rerun-if-changed={}", root.display());

    let lexers = configured(&root)
        .warnings(false)
        .extra_warnings(false)
        .debug(false)
        .file(root.join("generated").join("highlights.c"))
        .file(root.join("wasm2c").join("wasm-rt-impl.c"))
        .file(root.join("wasm2c").join("wasm-rt-mem-impl.c"))
        .compile_intermediates();

    let mut shim = configured(&root);
    shim.warnings(true).extra_warnings(true);
    if !shim.get_compiler().is_like_msvc() {
        shim.warnings_into_errors(true);
    }
    shim.objects(lexers)
        .file(root.join("shim").join("paneflow_highlights.c"))
        .compile("paneflow_highlights");
}

fn configured(root: &Path) -> cc::Build {
    let mut build = cc::Build::new();
    build
        .opt_level(2)
        .include(root)
        .include(root.join("wasm2c"))
        .include(root.join("generated"))
        .include(root.join("shim"));
    for (name, value) in RUNTIME_DEFINES {
        build.define(name, Some(*value));
    }
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("linux") => {
            build.define("_POSIX_C_SOURCE", Some("200809L"));
        }
        Ok("macos") => {
            build.define("_DARWIN_C_SOURCE", None);
        }
        _ => {}
    }
    if build.get_compiler().is_like_msvc() {
        build.flag("/std:c11");
    } else {
        build.flag("-std=c11");
    }
    build
}
