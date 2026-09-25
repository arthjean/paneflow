fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rustc-check-cfg=cfg(ghostty_native)");
    if std::env::var_os("CARGO_FEATURE_NATIVE").is_none() {
        return;
    }
    assert!(
        ghostty_native_target(),
        "paneflow-terminal-ghostty has no libghostty archive for target {}. Ghostty is the only \
         terminal backend, so this target cannot be built. Declare it in \
         native/libghostty/manifest.toml and produce its archive with the matching \
         scripts/build-libghostty-* recipe first.",
        std::env::var("TARGET").unwrap_or_else(|_| "<unknown>".to_owned())
    );
    println!("cargo::rustc-cfg=ghostty_native");
}

fn ghostty_native_target() -> bool {
    let cfg = |key: &str| std::env::var(key).unwrap_or_default();
    match cfg("CARGO_CFG_TARGET_OS").as_str() {
        "linux" => true,
        "macos" => cfg("CARGO_CFG_TARGET_ARCH") == "aarch64",
        "windows" => {
            cfg("CARGO_CFG_TARGET_ARCH") == "x86_64" && cfg("CARGO_CFG_TARGET_ENV") == "msvc"
        }
        _ => false,
    }
}
