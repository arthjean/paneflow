fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-env-changed=PANEFLOW_CEF_ROOT");
    println!("cargo:rerun-if-env-changed=CEF_PATH");
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
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var_os("CARGO_FEATURE_CEF_RUNTIME").is_some()
    {
        let root = std::path::PathBuf::from(
            std::env::var_os("PANEFLOW_CEF_ROOT")
                .ok_or("PANEFLOW_CEF_ROOT is required; run scripts/fetch-browser.py explicitly")?,
        );
        if !root.join("verified-manifest.sha256").is_file()
            || !root.join("Release/libcef.dll").is_file()
            || !root.join("cef_version.json").is_file()
        {
            return Err(
                "CEF runtime is absent or unverified; run scripts/fetch-browser.py explicitly"
                    .into(),
            );
        }
        let cef_path = std::path::PathBuf::from(
            std::env::var_os("CEF_PATH")
                .ok_or("CEF_PATH must point to the fetched Windows CEF runtime")?,
        );
        let root = root
            .canonicalize()
            .map_err(|error| format!("CEF runtime root is unreadable: {error}"))?;
        let cef_path = cef_path
            .canonicalize()
            .map_err(|error| format!("CEF_PATH is unreadable: {error}"))?;
        if root != cef_path {
            return Err("PANEFLOW_CEF_ROOT and CEF_PATH must refer to the same runtime".into());
        }
        let import_library = ["Release/libcef.lib", "libcef.lib"]
            .into_iter()
            .map(|path| root.join(path))
            .find(|path| path.is_file())
            .ok_or("CEF runtime has no libcef.lib import library")?;
        let link_directory = import_library
            .parent()
            .ok_or("CEF import library has no parent directory")?;
        println!(
            "cargo:rustc-link-search=native={}",
            link_directory.display()
        );
        let api_version = std::fs::read_to_string(root.join("include/cef_api_versions.h"))?
            .lines()
            .find_map(|line| {
                line.strip_prefix("#define CEF_API_VERSION_LAST ")?
                    .trim()
                    .strip_prefix("CEF_API_VERSION_")?
                    .parse::<u32>()
                    .ok()
            })
            .ok_or("CEF development package has no CEF_API_VERSION_LAST")?;
        let sdk_libs = [
            "comctl32.lib",
            "delayimp.lib",
            "mincore.lib",
            "powrprof.lib",
            "propsys.lib",
            "runtimeobject.lib",
            "setupapi.lib",
            "shcore.lib",
            "shell32.lib",
            "shlwapi.lib",
            "user32.lib",
            "version.lib",
            "wbemuuid.lib",
            "winmm.lib",
        ]
        .join(" ");
        let project_arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
            Ok("aarch64") => "arm64",
            _ => "x86_64",
        };
        let wrapper_root = root
            .to_string_lossy()
            .strip_prefix(r"\\?\")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| root.clone());
        let wrapper_output = cmake::Config::new(&wrapper_root)
            .generator("Ninja")
            .profile("RelWithDebInfo")
            .build_target("libcef_dll_wrapper")
            .define(
                "CEF_COMPILER_DEFINES",
                format!("CEF_API_VERSION={api_version}"),
            )
            .define("CMAKE_MSVC_RUNTIME_LIBRARY", "MultiThreaded")
            .define("CMAKE_OBJECT_PATH_MAX", "500")
            .define("CMAKE_STATIC_LINKER_FLAGS", &sdk_libs)
            .define("PROJECT_ARCH", project_arch)
            .define("USE_SANDBOX", "ON")
            .build();
        println!(
            "cargo:rustc-link-search=native={}/build/libcef_dll_wrapper",
            wrapper_output.display()
        );
        println!("cargo:rustc-link-lib=static=libcef_dll_wrapper");
        println!("cargo:rustc-link-lib=dylib=libcef");
    }
    Ok(())
}
