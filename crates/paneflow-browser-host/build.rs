fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-env-changed=PANEFLOW_CEF_ROOT");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux")
        && std::env::var_os("CARGO_FEATURE_CEF_RUNTIME").is_some()
    {
        let root = std::path::PathBuf::from(
            std::env::var_os("PANEFLOW_CEF_ROOT")
                .ok_or("PANEFLOW_CEF_ROOT is required; run scripts/fetch-browser.py explicitly")?,
        );
        if !root.join("verified-manifest.sha256").is_file()
            || !root.join("Release/libcef.so").is_file()
        {
            return Err(
                "CEF runtime is absent or unverified; run scripts/fetch-browser.py explicitly"
                    .into(),
            );
        }
        println!(
            "cargo:rustc-link-search=native={}",
            root.join("Release").display()
        );
        println!("cargo:rustc-link-lib=dylib=cef");
    }
    Ok(())
}
