fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os != "windows" || target_env != "msvc" {
        return;
    }
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let manifest = format!("{dir}\\paneflow-mcp-install.manifest");
    println!("cargo:rerun-if-changed={manifest}");
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{manifest}");
}
