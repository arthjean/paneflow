use cef::*;

pub const DISABLED_FEATURES: &str = "MediaRouter,DialMediaRouteProvider,GlobalMediaControls,OptimizationHints,OptimizationGuideModelDownloading,Translate,AutofillServerCommunication,CertificateTransparencyComponentUpdater,InterestFeedContentSuggestions,PrivacySandboxSettings4,SafeBrowsingEnhancedProtection";

pub const BROWSER_SWITCHES: &[&str] = &[
    "noerrdialogs",
    "no-first-run",
    "no-default-browser-check",
    "no-pings",
    "disable-extensions",
    "disable-component-extensions-with-background-pages",
    "disable-default-apps",
    "disable-background-networking",
    "disable-component-update",
    "disable-sync",
    "disable-domain-reliability",
    "disable-client-side-phishing-detection",
    "disable-breakpad",
    "safebrowsing-disable-auto-update",
    "metrics-recording-only",
    "use-mock-keychain",
];

pub fn engine_version() -> String {
    let raw = sys::CEF_VERSION;
    let text = std::str::from_utf8(raw).unwrap_or_default();
    format!("CEF {}", text.trim_end_matches('\0'))
}

pub fn headless_display() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none()
}

wrap_browser_process_handler! {
    struct HostProcess {}

    impl BrowserProcessHandler {
        fn on_already_running_app_relaunch(
            &self,
            _command_line: Option<&mut CommandLine>,
            _current_directory: Option<&CefString>,
        ) -> ::std::os::raw::c_int {
            1
        }
    }
}

wrap_app! {
    struct HostApp {}

    impl App {
        fn on_before_command_line_processing(
            &self,
            process_type: Option<&CefString>,
            command_line: Option<&mut CommandLine>,
        ) {
            let is_browser = process_type.is_none_or(|kind| kind.to_string().is_empty());
            let Some(command_line) = command_line else { return };
            if !is_browser {
                return;
            }
            for switch in BROWSER_SWITCHES {
                command_line.append_switch(Some(&(*switch).into()));
            }
            command_line.append_switch_with_value(
                Some(&"disable-features".into()),
                Some(&DISABLED_FEATURES.into()),
            );
            command_line.append_switch_with_value(
                Some(&"password-store".into()),
                Some(&"basic".into()),
            );
            if headless_display() {
                command_line.append_switch_with_value(
                    Some(&"ozone-platform".into()),
                    Some(&"headless".into()),
                );
            }
        }

        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(HostProcess::new())
        }
    }
}

pub fn host_app() -> App {
    HostApp::new()
}
