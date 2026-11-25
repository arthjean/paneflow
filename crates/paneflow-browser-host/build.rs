use std::path::Path;

fn main() {
    println!("cargo::rerun-if-env-changed=DEP_CEF_DLL_WRAPPER_CEF_DIR");
    let Some(sdk) = std::env::var_os("DEP_CEF_DLL_WRAPPER_CEF_DIR") else {
        eprintln!(
            "the cef-dll-sys build reported no CEF directory: run scripts/fetch-cef.sh, which places and unpacks the pinned CEF archive under native/cef/"
        );
        std::process::exit(1);
    };
    let sdk = Path::new(&sdk);
    if !sdk.join("archive.json").is_file() {
        eprintln!(
            "{} is not an unpacked CEF archive: run scripts/fetch-cef.sh",
            sdk.display()
        );
        std::process::exit(1);
    }
    println!("cargo::rustc-env=PANEFLOW_CEF_SDK_DIR={}", sdk.display());
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo::rustc-link-arg=-Wl,-rpath,$ORIGIN");
        println!("cargo::rustc-link-arg=-Wl,-rpath,{}", sdk.display());
    }
}
