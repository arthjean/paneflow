//! Build script for `paneflow-terminal-ghostty`.
//!
//! Its only job is to emit the `ghostty_native` cfg alias: the single
//! spelling of "the native Ghostty FFI layer is linked into this build". The
//! crate used to repeat a platform-and-feature predicate at every gate site;
//! the alias collapses those to one condition and makes adding a target a
//! change to [`ghostty_native_target`] rather than a textual sweep.
//!
//! It declares no build-dependencies and performs no filesystem or network
//! I/O, so it costs one process spawn and cannot fail.

fn main() {
    // The predicate depends only on this file plus the target and feature
    // set, and Cargo already reruns build scripts when either of those
    // changes. Naming the script keeps the rest of the crate out of the
    // rerun fingerprint.
    println!("cargo::rerun-if-changed=build.rs");
    // Printed unconditionally so that a build where the alias is *not* set
    // still declares it as a known cfg name and `unexpected_cfgs` stays quiet.
    println!("cargo::rustc-check-cfg=cfg(ghostty_native)");
    if std::env::var_os("CARGO_FEATURE_NATIVE").is_some() && ghostty_native_target() {
        println!("cargo::rustc-cfg=ghostty_native");
    }
}

/// Whether the target triple being built has a declared libghostty archive.
///
/// Kept in sync with the `[[target]]` entries of
/// `native/libghostty/manifest.toml` and with the matching predicate in
/// `paneflow-libghostty-sys` and `paneflow-app`. A target absent from this
/// list resolves to the `stub` backend with no `ghostty_native` cfg emitted,
/// which is also what keeps the optional `paneflow-libghostty-sys`
/// dependency - declared per-target in `Cargo.toml` - from being referenced
/// where it is not present.
///
/// Read from the `CARGO_CFG_TARGET_*` variables Cargo sets for the *target*
/// being built, never from `cfg!()`, which would describe the build host.
fn ghostty_native_target() -> bool {
    let cfg = |key: &str| std::env::var(key).unwrap_or_default();
    match cfg("CARGO_CFG_TARGET_OS").as_str() {
        "linux" => true,
        // Only Apple Silicon has a declared archive; x86_64-apple-darwin is a
        // closed release target and resolves to the Alacritty-only stub path.
        "macos" => cfg("CARGO_CFG_TARGET_ARCH") == "aarch64",
        "windows" => {
            cfg("CARGO_CFG_TARGET_ARCH") == "x86_64" && cfg("CARGO_CFG_TARGET_ENV") == "msvc"
        }
        _ => false,
    }
}
