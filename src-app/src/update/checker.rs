use std::time::Duration;

use semver::Version;

use super::install_method::{self, InstallMethod, PackageManager};
use crate::telemetry::event::UpdateAssetFormat;

const UPDATE_HTTP_TIMEOUT: Duration = Duration::from_secs(30);

const DEFAULT_FEED_URL: &str = "https://api.github.com/repos/arthjean/paneflow/releases/latest";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

const ALLOWED_UPDATE_HOSTS: &[&str] = &[
    "api.github.com",
    "github.com",
    "objects.githubusercontent.com",
];

pub(crate) fn update_feed_url() -> String {
    match std::env::var("PANEFLOW_UPDATE_FEED_URL") {
        Ok(v) if is_allowed_update_url(&v) => {
            log::warn!("update check: PANEFLOW_UPDATE_FEED_URL active → {v}");
            v
        }
        Ok(v) => {
            log::warn!(
                "update check: ignoring PANEFLOW_UPDATE_FEED_URL='{v}' (must be https:// to an allow-listed host, or loopback)"
            );
            DEFAULT_FEED_URL.to_string()
        }
        Err(_) => DEFAULT_FEED_URL.to_string(),
    }
}

fn is_allowed_update_url(url: &str) -> bool {
    is_allowed_update_url_impl(url, cfg!(debug_assertions))
}

fn is_allowed_update_url_impl(url: &str, allow_insecure_http: bool) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    let host = url_host(rest);
    if scheme.eq_ignore_ascii_case("https") {
        return is_loopback_host(host)
            || ALLOWED_UPDATE_HOSTS
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(host));
    }
    if scheme.eq_ignore_ascii_case("http") {
        return is_loopback_host(host) || allow_insecure_http;
    }
    false
}

fn url_host(after_scheme: &str) -> &str {
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    let host_port = match authority.rsplit_once('@') {
        Some((_userinfo, host)) => host,
        None => authority,
    };
    if let Some(after_bracket) = host_port.strip_prefix('[') {
        after_bracket.split(']').next().unwrap_or(after_bracket)
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    }
}

fn is_loopback_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

pub(crate) fn host_arch() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        if macos_is_translated() {
            return "aarch64";
        }
        std::env::consts::ARCH
    }
    #[cfg(target_os = "windows")]
    {
        windows_native_arch().unwrap_or(std::env::consts::ARCH)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::env::consts::ARCH
    }
}

#[cfg(target_os = "macos")]
fn macos_is_translated() -> bool {
    let mut ret: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    let rc = unsafe {
        libc::sysctlbyname(
            c"sysctl.proc_translated".as_ptr(),
            &mut ret as *mut libc::c_int as *mut libc::c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    rc == 0 && ret == 1
}

#[cfg(target_os = "windows")]
fn windows_native_arch() -> Option<&'static str> {
    use windows_sys::Win32::System::SystemInformation::{
        IMAGE_FILE_MACHINE_AMD64, IMAGE_FILE_MACHINE_ARM64, IMAGE_FILE_MACHINE_I386,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, IsWow64Process2};

    let mut process_machine: u16 = 0;
    let mut native_machine: u16 = 0;
    let ok = unsafe {
        IsWow64Process2(
            GetCurrentProcess(),
            &mut process_machine,
            &mut native_machine,
        )
    };
    if ok == 0 {
        return None;
    }
    match native_machine {
        IMAGE_FILE_MACHINE_ARM64 => Some("aarch64"),
        IMAGE_FILE_MACHINE_AMD64 => Some("x86_64"),
        IMAGE_FILE_MACHINE_I386 => Some("x86"),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetFormat {
    Deb,
    Rpm,
    AppImage,
    TarGz,
    Dmg,
    Msi,
}

impl AssetFormat {
    pub(crate) fn telemetry_value(&self) -> UpdateAssetFormat {
        match self {
            AssetFormat::Deb => UpdateAssetFormat::Deb,
            AssetFormat::Rpm => UpdateAssetFormat::Rpm,
            AssetFormat::AppImage => UpdateAssetFormat::AppImage,
            AssetFormat::TarGz => UpdateAssetFormat::TarGz,
            AssetFormat::Dmg => UpdateAssetFormat::Dmg,
            AssetFormat::Msi => UpdateAssetFormat::Msi,
        }
    }

    fn filename_suffix(&self) -> &'static str {
        match self {
            AssetFormat::Deb => ".deb",
            AssetFormat::Rpm => ".rpm",
            AssetFormat::AppImage => ".AppImage",
            AssetFormat::TarGz => ".tar.gz",
            AssetFormat::Dmg => ".dmg",
            AssetFormat::Msi => ".msi",
        }
    }

    fn target_qualifier(&self) -> &'static str {
        match self {
            AssetFormat::Dmg => "-apple-darwin",
            AssetFormat::Msi => "-pc-windows-msvc",
            _ => "",
        }
    }

    fn from_install_method(method: &InstallMethod) -> Self {
        match method {
            InstallMethod::SystemPackage {
                manager: PackageManager::Apt,
            } => AssetFormat::Deb,
            InstallMethod::SystemPackage {
                manager: PackageManager::Dnf,
            }
            | InstallMethod::SystemPackage {
                manager: PackageManager::Zypper,
            } => AssetFormat::Rpm,
            InstallMethod::SystemPackage {
                manager: PackageManager::Other,
            }
            | InstallMethod::SystemPackage {
                manager: PackageManager::RpmOstree,
            } => AssetFormat::TarGz,
            InstallMethod::AppImage { .. } => AssetFormat::AppImage,
            InstallMethod::TarGz { .. } => AssetFormat::TarGz,
            InstallMethod::AppBundle { .. } => AssetFormat::Dmg,
            InstallMethod::WindowsMsi { .. } => AssetFormat::Msi,
            InstallMethod::ExternallyManaged { .. } => AssetFormat::TarGz,
            InstallMethod::Unknown => AssetFormat::TarGz,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum UpdateStatus {
    Checking,
    Available {
        version: String,
        url: String,
        asset_url: Option<String>,
        asset_format: Option<AssetFormat>,
    },
    UpToDate,
    Failed,
}

pub type SharedUpdateSlot = std::sync::Arc<std::sync::Mutex<Option<UpdateStatus>>>;

pub fn spawn_check(
    telemetry: std::sync::Arc<crate::telemetry::client::TelemetryClient>,
) -> SharedUpdateSlot {
    let slot: SharedUpdateSlot =
        std::sync::Arc::new(std::sync::Mutex::new(Some(UpdateStatus::Checking)));
    let writer = std::sync::Arc::clone(&slot);
    std::thread::spawn(move || {
        crate::app::telemetry_events::emit_update_check_started(&telemetry, CURRENT_VERSION);
        let status = check_github_release(&telemetry);
        *writer.lock().unwrap_or_else(|e| e.into_inner()) = Some(status);
    });
    slot
}

#[derive(serde::Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    assets: Vec<GitHubAsset>,
}

#[derive(serde::Deserialize)]
pub(crate) struct GitHubAsset {
    pub(crate) name: String,
    pub(crate) browser_download_url: String,
}

pub fn pick_asset<'a>(
    assets: &'a [GitHubAsset],
    arch: &str,
    method: InstallMethod,
) -> Option<&'a GitHubAsset> {
    let format = AssetFormat::from_install_method(&method);
    let expected = format!(
        "-{arch}{}{}",
        format.target_qualifier(),
        format.filename_suffix()
    )
    .to_ascii_lowercase();
    let picked = assets
        .iter()
        .find(|a| a.name.to_ascii_lowercase().ends_with(&expected))?;
    if !is_allowed_update_url(&picked.browser_download_url) {
        log::warn!(
            "update check: asset '{}' has a disallowed download URL ({}) - ignoring",
            picked.name,
            picked.browser_download_url
        );
        return None;
    }
    Some(picked)
}

fn transient_update_error(e: &ureq::Error) -> bool {
    match e {
        ureq::Error::StatusCode(code) => *code == 408 || *code == 429 || (500..600).contains(code),
        _ => true,
    }
}

pub(crate) fn check_github_release(
    telemetry: &crate::telemetry::client::TelemetryClient,
) -> UpdateStatus {
    #[cfg(debug_assertions)]
    if let Ok(forced_version) = std::env::var("PANEFLOW_DEV_FORCE_UPDATE") {
        let version = forced_version.trim().trim_start_matches('v').to_string();
        if !version.is_empty() && Version::parse(&version).is_ok() {
            log::warn!("update check: PANEFLOW_DEV_FORCE_UPDATE active, faking v{version}");
            return UpdateStatus::Available {
                version,
                url: "https://github.com/arthjean/paneflow/releases".to_string(),
                asset_url: None,
                asset_format: None,
            };
        }
    }

    let feed_url = update_feed_url();
    let response = ureq::get(&feed_url)
        .config()
        .timeout_global(Some(UPDATE_HTTP_TIMEOUT))
        .build()
        .header("User-Agent", &format!("paneflow/{CURRENT_VERSION}"))
        .header("Accept", "application/vnd.github.v3+json")
        .call();

    let mut response = match response {
        Ok(r) => r,
        Err(e) => {
            if transient_update_error(&e) {
                log::debug!("update check skipped (transient): {e}");
            } else {
                log::warn!("update check failed: {e}");
            }
            return UpdateStatus::Failed;
        }
    };

    let release: GitHubRelease = match response.body_mut().read_json() {
        Ok(r) => r,
        Err(e) => {
            log::warn!("update check: failed to parse response: {e}");
            return UpdateStatus::Failed;
        }
    };

    let remote_tag = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name);

    let remote = match Version::parse(remote_tag) {
        Ok(v) => v,
        Err(e) => {
            log::warn!(
                "update check: invalid remote version '{}': {e}",
                release.tag_name
            );
            return UpdateStatus::Failed;
        }
    };
    let local = match Version::parse(CURRENT_VERSION) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("update check: invalid local version '{CURRENT_VERSION}': {e}");
            return UpdateStatus::Failed;
        }
    };

    if remote > local {
        let method = install_method::detect();
        let picked = pick_asset(&release.assets, host_arch(), method.clone());
        let (asset_url, asset_format) = match picked {
            Some(asset) => (
                Some(asset.browser_download_url.clone()),
                Some(AssetFormat::from_install_method(&method)),
            ),
            None => (None, None),
        };
        log::info!(
            "update available: v{remote} (current: v{local}) - asset_format: {asset_format:?}"
        );
        if let Some(format) = asset_format.as_ref() {
            crate::app::telemetry_events::emit_update_available(
                telemetry,
                CURRENT_VERSION,
                &remote.to_string(),
                format.telemetry_value(),
            );
        }
        UpdateStatus::Available {
            version: remote.to_string(),
            url: release.html_url,
            asset_url,
            asset_format,
        }
    } else {
        log::info!("up to date (v{local})");
        UpdateStatus::UpToDate
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_asset(name: &str) -> GitHubAsset {
        GitHubAsset {
            name: name.to_string(),
            browser_download_url: format!(
                "https://github.com/arthjean/paneflow/releases/download/v0/{name}"
            ),
        }
    }

    fn apt() -> InstallMethod {
        InstallMethod::SystemPackage {
            manager: PackageManager::Apt,
        }
    }
    fn dnf() -> InstallMethod {
        InstallMethod::SystemPackage {
            manager: PackageManager::Dnf,
        }
    }

    fn zypper() -> InstallMethod {
        InstallMethod::SystemPackage {
            manager: PackageManager::Zypper,
        }
    }
    fn tar_gz() -> InstallMethod {
        InstallMethod::TarGz {
            app_dir: PathBuf::from("/home/u/.local/paneflow.app"),
        }
    }
    fn app_image() -> InstallMethod {
        InstallMethod::AppImage {
            mount_point: PathBuf::from("/tmp/.mount_x"),
            source_path: PathBuf::from("/home/u/Downloads/paneflow.AppImage"),
        }
    }
    fn app_bundle() -> InstallMethod {
        InstallMethod::AppBundle {
            bundle_path: PathBuf::from("/Applications/PaneFlow.app"),
        }
    }
    fn windows_msi() -> InstallMethod {
        InstallMethod::WindowsMsi {
            install_path: PathBuf::from("C:/Program Files/PaneFlow"),
        }
    }

    #[test]
    fn url_host_extracts_authority() {
        assert_eq!(url_host("api.github.com/repos/x"), "api.github.com");
        assert_eq!(url_host("api.github.com:443/x"), "api.github.com");
        assert_eq!(url_host("127.0.0.1:8080/latest"), "127.0.0.1");
        assert_eq!(url_host("[::1]:9000/latest"), "::1");
        assert_eq!(url_host("api.github.com@evil.com/x"), "evil.com");
        assert_eq!(url_host("github.com"), "github.com");
    }

    #[test]
    fn https_allowlisted_host_allowed_in_release() {
        assert!(is_allowed_update_url_impl(
            "https://api.github.com/repos/arthjean/paneflow/releases/latest",
            false
        ));
        assert!(is_allowed_update_url_impl(
            "HTTPS://API.GITHUB.COM/repos/arthjean/paneflow/releases/latest",
            false
        ));
        assert!(is_allowed_update_url_impl(
            "https://github.com/arthjean/paneflow/releases/download/v1/x.tar.gz",
            false
        ));
    }

    #[test]
    fn https_offdomain_host_rejected() {
        assert!(!is_allowed_update_url_impl(
            "https://evil.com/latest",
            false
        ));
        assert!(!is_allowed_update_url_impl(
            "https://api.github.com.evil.com/latest",
            false
        ));
        assert!(!is_allowed_update_url_impl(
            "https://api.github.com@evil.com/latest",
            false
        ));
    }

    #[test]
    fn plain_http_nonloopback_is_release_rejected_debug_allowed() {
        assert!(!is_allowed_update_url_impl("http://evil.com/latest", false));
        assert!(is_allowed_update_url_impl("http://evil.com/latest", true));
    }

    #[test]
    fn loopback_http_allowed_in_all_builds() {
        for url in [
            "http://127.0.0.1:8080/latest",
            "http://localhost:9000/latest",
            "http://LOCALHOST:9000/latest",
            "http://127.0.0.1:1/latest",
        ] {
            assert!(
                is_allowed_update_url_impl(url, false),
                "loopback must be allowed: {url}"
            );
        }
    }

    #[test]
    fn host_arch_falls_back_to_compile_arch_on_linux() {
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        assert_eq!(host_arch(), std::env::consts::ARCH);
        assert!(!host_arch().is_empty());
    }

    #[test]
    fn non_http_scheme_rejected() {
        assert!(!is_allowed_update_url_impl("ftp://api.github.com/x", true));
        assert!(!is_allowed_update_url_impl("file:///etc/passwd", true));
        assert!(!is_allowed_update_url_impl("api.github.com/x", true));
    }

    #[test]
    fn pick_asset_drops_offdomain_download_url() {
        let assets = vec![GitHubAsset {
            name: "paneflow-0.3.9-x86_64.tar.gz".to_string(),
            browser_download_url: "https://evil.example/paneflow-0.3.9-x86_64.tar.gz".to_string(),
        }];
        assert!(
            pick_asset(&assets, "x86_64", tar_gz()).is_none(),
            "off-domain asset URL must be rejected"
        );
    }

    #[test]
    fn apt_picks_deb() {
        let assets = vec![
            make_asset("paneflow-v0.2.0-x86_64.deb"),
            make_asset("paneflow-v0.2.0-x86_64.tar.gz"),
            make_asset("paneflow-v0.2.0-x86_64.AppImage"),
        ];
        let r = pick_asset(&assets, "x86_64", apt());
        assert_eq!(
            r.map(|a| a.name.as_str()),
            Some("paneflow-v0.2.0-x86_64.deb")
        );
    }

    #[test]
    fn dnf_picks_rpm() {
        let assets = vec![
            make_asset("paneflow-v0.2.0-x86_64.rpm"),
            make_asset("paneflow-v0.2.0-x86_64.deb"),
            make_asset("paneflow-v0.2.0-x86_64.tar.gz"),
        ];
        let r = pick_asset(&assets, "x86_64", dnf());
        assert_eq!(
            r.map(|a| a.name.as_str()),
            Some("paneflow-v0.2.0-x86_64.rpm")
        );
    }

    #[test]
    fn zypper_picks_rpm() {
        let assets = vec![
            make_asset("paneflow-v0.2.0-x86_64.rpm"),
            make_asset("paneflow-v0.2.0-x86_64.deb"),
            make_asset("paneflow-v0.2.0-x86_64.tar.gz"),
        ];
        let r = pick_asset(&assets, "x86_64", zypper());
        assert_eq!(
            r.map(|a| a.name.as_str()),
            Some("paneflow-v0.2.0-x86_64.rpm")
        );
    }

    #[test]
    fn appimage_method_picks_appimage() {
        let assets = vec![
            make_asset("paneflow-v0.2.0-x86_64.AppImage"),
            make_asset("paneflow-v0.2.0-x86_64.deb"),
        ];
        let r = pick_asset(&assets, "x86_64", app_image());
        assert_eq!(
            r.map(|a| a.name.as_str()),
            Some("paneflow-v0.2.0-x86_64.AppImage")
        );
    }

    #[test]
    fn tar_gz_method_picks_tar_gz() {
        let assets = vec![
            make_asset("paneflow-v0.2.0-x86_64.tar.gz"),
            make_asset("paneflow-v0.2.0-x86_64.deb"),
        ];
        let r = pick_asset(&assets, "x86_64", tar_gz());
        assert_eq!(
            r.map(|a| a.name.as_str()),
            Some("paneflow-v0.2.0-x86_64.tar.gz")
        );
    }

    #[test]
    fn tar_gz_method_picks_tar_gz_aarch64() {
        let assets = vec![
            make_asset("paneflow-v0.2.0-x86_64.tar.gz"),
            make_asset("paneflow-v0.2.0-x86_64.deb"),
            make_asset("paneflow-v0.2.0-aarch64.tar.gz"),
            make_asset("paneflow-v0.2.0-aarch64.deb"),
        ];
        let r = pick_asset(&assets, "aarch64", tar_gz());
        assert_eq!(
            r.map(|a| a.name.as_str()),
            Some("paneflow-v0.2.0-aarch64.tar.gz")
        );
    }

    #[test]
    fn unknown_method_falls_back_to_tar_gz() {
        let assets = vec![
            make_asset("paneflow-v0.2.0-x86_64.tar.gz"),
            make_asset("paneflow-v0.2.0-x86_64.deb"),
            make_asset("paneflow-v0.2.0-x86_64.AppImage"),
        ];
        let r = pick_asset(&assets, "x86_64", InstallMethod::Unknown);
        assert_eq!(
            r.map(|a| a.name.as_str()),
            Some("paneflow-v0.2.0-x86_64.tar.gz")
        );
    }

    #[test]
    fn fedora_never_handed_deb_fallback() {
        let assets = vec![
            make_asset("paneflow-v0.2.0-x86_64.deb"),
            make_asset("paneflow-v0.2.0-x86_64.tar.gz"),
        ];
        let r = pick_asset(&assets, "x86_64", dnf());
        assert!(r.is_none(), "Fedora user must NOT receive a .deb");
    }

    #[test]
    fn multi_arch_release_picks_correct_arch() {
        let assets = vec![
            make_asset("paneflow-v0.2.0-aarch64.deb"),
            make_asset("paneflow-v0.2.0-x86_64.deb"),
        ];
        let x = pick_asset(&assets, "x86_64", apt());
        assert_eq!(
            x.map(|a| a.name.as_str()),
            Some("paneflow-v0.2.0-x86_64.deb")
        );
        let a = pick_asset(&assets, "aarch64", apt());
        assert_eq!(
            a.map(|a| a.name.as_str()),
            Some("paneflow-v0.2.0-aarch64.deb")
        );
    }

    #[test]
    fn match_is_case_insensitive() {
        let assets = vec![make_asset("PaneFlow-v0.2.0-X86_64.DEB")];
        let r = pick_asset(&assets, "x86_64", apt());
        assert!(r.is_some(), "case-insensitive match failed");
    }

    #[test]
    fn match_is_v_prefix_agnostic() {
        let legacy = vec![make_asset("paneflow-v0.2.10-x86_64.deb")];
        let current = vec![make_asset("paneflow-0.3.0-x86_64.deb")];
        assert_eq!(
            pick_asset(&legacy, "x86_64", apt()).map(|a| a.name.as_str()),
            Some("paneflow-v0.2.10-x86_64.deb"),
            "legacy v-prefixed asset must match",
        );
        assert_eq!(
            pick_asset(&current, "x86_64", apt()).map(|a| a.name.as_str()),
            Some("paneflow-0.3.0-x86_64.deb"),
            "current non-v-prefixed asset must match",
        );

        let mixed = vec![
            make_asset("paneflow-v0.2.10-x86_64.deb"),
            make_asset("paneflow-0.3.0-x86_64.deb"),
        ];
        assert!(
            pick_asset(&mixed, "x86_64", apt()).is_some(),
            "mixed-format release must yield at least one match",
        );
    }

    #[test]
    fn returns_none_when_no_matching_asset() {
        let assets = vec![
            make_asset("README.md"),
            make_asset("paneflow-v0.2.0-x86_64.AppImage.zsync"),
        ];
        let r = pick_asset(&assets, "x86_64", tar_gz());
        assert!(r.is_none());
    }

    #[test]
    fn zsync_sidecar_never_picked_for_appimage() {
        let assets = vec![
            make_asset("paneflow-v0.2.0-x86_64.AppImage.zsync"),
            make_asset("paneflow-v0.2.0-x86_64.AppImage"),
        ];
        let r = pick_asset(&assets, "x86_64", app_image());
        assert_eq!(
            r.map(|a| a.name.as_str()),
            Some("paneflow-v0.2.0-x86_64.AppImage")
        );
    }

    #[test]
    fn format_from_install_method_mapping() {
        assert_eq!(AssetFormat::from_install_method(&apt()), AssetFormat::Deb);
        assert_eq!(AssetFormat::from_install_method(&dnf()), AssetFormat::Rpm);
        assert_eq!(
            AssetFormat::from_install_method(&zypper()),
            AssetFormat::Rpm
        );
        assert_eq!(
            AssetFormat::from_install_method(&tar_gz()),
            AssetFormat::TarGz
        );
        assert_eq!(
            AssetFormat::from_install_method(&app_image()),
            AssetFormat::AppImage
        );
        assert_eq!(
            AssetFormat::from_install_method(&InstallMethod::Unknown),
            AssetFormat::TarGz
        );
        assert_eq!(
            AssetFormat::from_install_method(&app_bundle()),
            AssetFormat::Dmg
        );
    }

    #[test]
    fn app_bundle_picks_dmg_aarch64() {
        let assets = vec![
            make_asset("paneflow-0.2.0-aarch64-apple-darwin.dmg"),
            make_asset("paneflow-0.2.0-x86_64-apple-darwin.dmg"),
            make_asset("paneflow-0.2.0-aarch64.tar.gz"),
        ];
        let r = pick_asset(&assets, "aarch64", app_bundle());
        assert_eq!(
            r.map(|a| a.name.as_str()),
            Some("paneflow-0.2.0-aarch64-apple-darwin.dmg")
        );
    }

    #[test]
    fn app_bundle_picks_dmg_x86_64() {
        let assets = vec![
            make_asset("paneflow-0.2.0-aarch64-apple-darwin.dmg"),
            make_asset("paneflow-0.2.0-x86_64-apple-darwin.dmg"),
            make_asset("paneflow-0.2.0-x86_64.deb"),
        ];
        let r = pick_asset(&assets, "x86_64", app_bundle());
        assert_eq!(
            r.map(|a| a.name.as_str()),
            Some("paneflow-0.2.0-x86_64-apple-darwin.dmg")
        );
    }

    #[test]
    fn app_bundle_returns_none_when_release_has_no_dmg() {
        let assets = vec![
            make_asset("paneflow-0.2.0-x86_64.deb"),
            make_asset("paneflow-0.2.0-aarch64.tar.gz"),
            make_asset("paneflow-0.2.0-x86_64.AppImage"),
        ];
        let r = pick_asset(&assets, "aarch64", app_bundle());
        assert!(
            r.is_none(),
            "AppBundle user must NOT be handed a Linux asset"
        );
    }

    #[test]
    fn linux_never_picks_dmg() {
        let assets = vec![
            make_asset("paneflow-0.2.0-aarch64-apple-darwin.dmg"),
            make_asset("paneflow-0.2.0-aarch64.deb"),
        ];
        let r = pick_asset(&assets, "aarch64", apt());
        assert_eq!(
            r.map(|a| a.name.as_str()),
            Some("paneflow-0.2.0-aarch64.deb")
        );
    }

    #[test]
    fn dmg_match_is_case_insensitive() {
        let assets = vec![make_asset("PaneFlow-0.2.0-AArch64-Apple-Darwin.DMG")];
        let r = pick_asset(&assets, "aarch64", app_bundle());
        assert!(r.is_some(), "case-insensitive .dmg match failed");
    }

    #[test]
    fn dmg_arch_mismatch_returns_none() {
        let assets = vec![make_asset("paneflow-0.2.0-aarch64-apple-darwin.dmg")];
        let r = pick_asset(&assets, "x86_64", app_bundle());
        assert!(r.is_none());
    }

    #[test]
    fn windows_msi_picks_msi_x86_64() {
        let assets = vec![
            make_asset("paneflow-0.2.0-x86_64-pc-windows-msvc.msi"),
            make_asset("paneflow-0.2.0-x86_64.deb"),
            make_asset("paneflow-0.2.0-x86_64-apple-darwin.dmg"),
        ];
        let r = pick_asset(&assets, "x86_64", windows_msi());
        assert_eq!(
            r.map(|a| a.name.as_str()),
            Some("paneflow-0.2.0-x86_64-pc-windows-msvc.msi")
        );
    }

    #[test]
    fn windows_msi_returns_none_when_release_has_no_msi() {
        let assets = vec![
            make_asset("paneflow-0.2.0-x86_64.deb"),
            make_asset("paneflow-0.2.0-x86_64.tar.gz"),
            make_asset("paneflow-0.2.0-x86_64.AppImage"),
        ];
        let r = pick_asset(&assets, "x86_64", windows_msi());
        assert!(
            r.is_none(),
            "WindowsMsi user must NOT be handed a Linux/macOS asset"
        );
    }

    #[test]
    fn linux_never_picks_msi() {
        let assets = vec![
            make_asset("paneflow-0.2.0-x86_64-pc-windows-msvc.msi"),
            make_asset("paneflow-0.2.0-x86_64.deb"),
        ];
        let r = pick_asset(&assets, "x86_64", apt());
        assert_eq!(
            r.map(|a| a.name.as_str()),
            Some("paneflow-0.2.0-x86_64.deb")
        );
    }

    #[test]
    fn msi_match_is_case_insensitive() {
        let assets = vec![make_asset("PaneFlow-0.2.0-X86_64-PC-Windows-Msvc.MSI")];
        let r = pick_asset(&assets, "x86_64", windows_msi());
        assert!(r.is_some(), "case-insensitive .msi match failed");
    }

    use crate::app::telemetry_events::{
        emit_update_available, emit_update_check_started, emit_update_dismissed_via,
    };
    use crate::telemetry::client::TelemetryClient;

    #[test]
    fn asset_formats_map_to_closed_telemetry_values() {
        let cases = [
            (AssetFormat::Deb, UpdateAssetFormat::Deb),
            (AssetFormat::Rpm, UpdateAssetFormat::Rpm),
            (AssetFormat::AppImage, UpdateAssetFormat::AppImage),
            (AssetFormat::TarGz, UpdateAssetFormat::TarGz),
            (AssetFormat::Dmg, UpdateAssetFormat::Dmg),
            (AssetFormat::Msi, UpdateAssetFormat::Msi),
        ];
        for (format, expected) in cases {
            assert_eq!(format.telemetry_value(), expected);
        }
    }

    #[test]
    fn update_available_skipped_when_no_asset_matches() {
        let assets = vec![make_asset("paneflow-0.2.12-x86_64.deb")];
        let picked = pick_asset(&assets, "x86_64", dnf());
        assert!(
            picked.is_none(),
            "dnf user should see no .deb asset → no update_available emit"
        );
    }

    #[test]
    fn disabled_client_emits_are_no_ops() {
        let disabled = TelemetryClient::disabled();
        assert!(!disabled.is_active());
        emit_update_check_started(&disabled, "0.2.11");
        emit_update_available(&disabled, "0.2.11", "0.2.12", UpdateAssetFormat::TarGz);
        emit_update_dismissed_via(&disabled, "0.2.11", "0.2.12");
    }
}
