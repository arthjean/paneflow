use super::*;

#[cfg(target_os = "macos")]
use gpui::point;

fn should_load_login_shell_env_for_startup(
    is_msi_relay: bool,
    is_mcp_subcommand: bool,
    is_cli_subcommand: bool,
    is_hooks_subcommand: bool,
    is_update_and_exit: bool,
    is_unknown_verb: bool,
) -> bool {
    !(is_msi_relay
        || is_mcp_subcommand
        || is_cli_subcommand
        || is_hooks_subcommand
        || is_update_and_exit
        || is_unknown_verb)
}

fn should_extract_mcp_bridge_for_cli(args: &[String]) -> bool {
    args.get(1).map(String::as_str) == Some("mcp")
        && args.get(2).map(String::as_str) == Some("install")
        && args.len() == 3
}

fn extract_integration_binaries(
    command: &str,
) -> Option<paneflow_mcp_install::IntegrationBinaries> {
    match (
        ai_hooks::extract::ensure_ai_hook_extracted(),
        ai_hooks::extract::ensure_bridge_extracted(),
    ) {
        (Ok(hook_binary), Ok(bridge_binary)) => Some(paneflow_mcp_install::IntegrationBinaries {
            hook_binary,
            bridge_binary,
        }),
        (hook, bridge) => {
            if let Err(error) = hook {
                eprintln!("{command}: hook extraction failed: {error:#}");
            }
            if let Err(error) = bridge {
                eprintln!("{command}: bridge extraction failed: {error:#}");
            }
            None
        }
    }
}

fn run_update_and_exit() -> i32 {
    use crate::update::checker::{UpdateStatus, check_github_release};
    use crate::update::install_method::{self, InstallMethod};

    let method = install_method::detect();
    log::info!("--update-and-exit: install method = {method:?}");

    let null_telemetry = crate::telemetry::client::TelemetryClient::disabled();
    let status = check_github_release(&null_telemetry);
    let (version, asset_url) = match status {
        UpdateStatus::Available {
            version,
            asset_url: Some(url),
            ..
        } => (version, url),
        UpdateStatus::Available {
            asset_url: None, ..
        } => {
            eprintln!("paneflow-update: no asset matched the install method - nothing to install");
            return 5;
        }
        UpdateStatus::UpToDate => {
            eprintln!("paneflow-update: already up to date");
            return 2;
        }
        UpdateStatus::Failed => {
            eprintln!(
                "paneflow-update: feed unreachable at {} - check PANEFLOW_UPDATE_FEED_URL",
                crate::update::checker::update_feed_url()
            );
            return 3;
        }
        UpdateStatus::Checking => {
            eprintln!("paneflow-update: checker returned Checking - should never happen");
            return 1;
        }
    };

    log::info!("--update-and-exit: installing v{version} from {asset_url}");

    match method {
        InstallMethod::TarGz { .. } => match crate::update::linux::targz::run_update(&asset_url) {
            Ok(new_bin) => {
                println!("paneflow-update: ok new={}", new_bin.display());
                0
            }
            Err(err) => {
                let classified = crate::update::error::UpdateError::classify(&err);
                if matches!(
                    classified,
                    crate::update::error::UpdateError::IntegrityMismatch { .. }
                ) {
                    eprintln!("paneflow-update: hash mismatch - {err}");
                    return 4;
                }
                eprintln!("paneflow-update: install failed - {err}");
                1
            }
        },
        InstallMethod::AppImage { source_path, .. } => {
            match crate::update::linux::appimage::run_update(&source_path, &asset_url) {
                Ok(new_bin) => {
                    println!("paneflow-update: ok new={}", new_bin.display());
                    0
                }
                Err(err) => {
                    eprintln!("paneflow-update: AppImage install failed - {err}");
                    1
                }
            }
        }
        other => {
            eprintln!(
                "paneflow-update: --update-and-exit does not support install method {other:?}"
            );
            5
        }
    }
}

#[cfg(windows)]
fn should_detach_windows_console(
    is_scriptable_invocation: bool,
    console_process_count: u32,
) -> bool {
    !is_scriptable_invocation && console_process_count == 1
}

#[cfg(windows)]
fn detach_lonely_windows_console_for_gui_launch(is_scriptable_invocation: bool) {
    use windows_sys::Win32::System::Console::{FreeConsole, GetConsoleProcessList};

    let mut processes = [0_u32; 2];
    let count = unsafe { GetConsoleProcessList(processes.as_mut_ptr(), processes.len() as u32) };
    if should_detach_windows_console(is_scriptable_invocation, count) {
        unsafe {
            let _ = FreeConsole();
        }
    }
}

pub(crate) fn run() {
    startup_trace::begin();
    let args: Vec<String> = std::env::args().collect();
    for migrated in paneflow_home::migrate_legacy_home() {
        eprintln!("paneflow: moved user state to {}", migrated.display());
    }
    startup_trace::mark("home_migrated");
    #[cfg(target_os = "windows")]
    if external_open::is_open_url_helper_invocation(&args) {
        std::process::exit(external_open::run_open_url_helper_from_args(&args));
    }
    #[cfg(windows)]
    let is_msi_relay = update::windows::msi::is_relay_invocation(&args);
    #[cfg(not(windows))]
    let is_msi_relay = false;
    let is_mcp_subcommand = args.get(1).map(String::as_str) == Some("mcp");
    let is_cli_subcommand = cli::is_cli_verb(args.get(1).map(String::as_str));
    let is_hooks_subcommand = args.get(1).map(String::as_str) == Some("hooks");
    let is_integrations_subcommand = args.get(1).map(String::as_str) == Some("integrations");
    let is_hook_utility_subcommand = is_hooks_subcommand || is_integrations_subcommand;
    let is_global_help = !is_msi_relay
        && !is_mcp_subcommand
        && !is_cli_subcommand
        && !is_hook_utility_subcommand
        && args.iter().any(|a| a == "--help" || a == "-h");
    let is_global_version = !is_msi_relay
        && !is_mcp_subcommand
        && !is_cli_subcommand
        && !is_hook_utility_subcommand
        && args.iter().any(|a| a == "--version" || a == "-v");
    let is_update_and_exit = !is_msi_relay
        && !is_mcp_subcommand
        && !is_cli_subcommand
        && !is_hook_utility_subcommand
        && args.iter().any(|a| a == "--update-and-exit");
    let is_unknown_verb = args
        .get(1)
        .is_some_and(|verb| cli::looks_like_unknown_verb(Some(verb.as_str())));

    #[cfg(windows)]
    detach_lonely_windows_console_for_gui_launch(
        is_msi_relay
            || is_mcp_subcommand
            || is_cli_subcommand
            || is_hook_utility_subcommand
            || is_global_help
            || is_global_version
            || is_update_and_exit
            || is_unknown_verb,
    );

    #[cfg(windows)]
    if is_msi_relay {
        std::process::exit(update::windows::msi::run_relay_from_args(&args));
    }

    if is_global_help {
        println!(
            "PaneFlow {version} - native terminal workspace for coding agents\n\
             \n\
             Usage: paneflow [OPTIONS]\n\
             \x20      paneflow mcp <install|status|uninstall>\n\
             \x20      paneflow integrations <list|install|remove>\n\
             \x20      paneflow host <start|status|stop>\n\
             \x20      paneflow serve <start|status|stop>\n\
             \n\
             Options:\n\
             \x20 -h, --help       Print this help message\n\
             \x20 -v, --version    Print version\n\
             \x20 --update-and-exit  Check for an update and exit (CI harness)\n\
             \n\
             Agent workflow:\n\
             \x20 Launch Claude Code, Codex, opencode, Pi, or any CLI agent in panes\n\
             \x20 Use `paneflow mcp install` so capable agents can read pane output\n\
             \n\
             Keybindings:\n\
             \x20 Ctrl+Shift+D/E   Split horizontal/vertical\n\
             \x20 Ctrl+Shift+W     Close pane\n\
             \x20 Alt+Arrow        Focus adjacent pane\n\
             \x20 Ctrl+Shift+N     New workspace\n\
             \x20 Ctrl+Tab         Next workspace\n\
             \x20 Ctrl+1-9         Switch to workspace N\n\
             \n\
             Config paths and IPC endpoints are documented in the README.\n\
             https://github.com/arthjean/paneflow",
            version = env!("CARGO_PKG_VERSION")
        );
        return;
    }
    if is_global_version {
        println!("paneflow {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    unsafe { agents::parent_guard::scrub_claudecode_env_before_threads() };

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(
        "warn,wgpu_hal=off,naga=warn,gpui_macos::text_system=error,zbus=warn,zbus::proxy=error,tracing::span=warn",
    ))
    .init();

    match agents::parent_guard::install_process_job() {
        Ok(agents::parent_guard::ParentGuardStatus::Installed) => {}
        Ok(agents::parent_guard::ParentGuardStatus::Unsupported) => {
            log::debug!(
                "parent_guard: process-wide job guard unsupported on Unix; shim-wrapped agents use shim guards"
            );
        }
        Err(err) => {
            log::warn!(
                "parent_guard: failed to install Job Object; kill -9 of Paneflow may orphan agent CLIs ({err})"
            );
        }
    }
    startup_trace::mark("process_job_installed");

    if should_load_login_shell_env_for_startup(
        is_msi_relay,
        is_mcp_subcommand,
        is_cli_subcommand,
        is_hook_utility_subcommand,
        is_update_and_exit,
        is_unknown_verb,
    ) {
        login_shell_env::load_login_shell_env();
    }
    startup_trace::mark("login_shell_env_loaded");

    runtime_paths::augment_path_for_gui_launch();

    if is_update_and_exit {
        std::process::exit(run_update_and_exit());
    }

    if args.get(1).map(String::as_str) == Some("mcp") {
        let bridge_path = if should_extract_mcp_bridge_for_cli(&args) {
            match ai_hooks::extract::ensure_bridge_extracted() {
                Ok(p) => Some(p),
                Err(e) => {
                    log::warn!("paneflow mcp: bridge extraction failed ({e:#})");
                    runtime_paths::bridge_binary_path()
                }
            }
        } else {
            runtime_paths::bridge_binary_path()
        };
        std::process::exit(paneflow_mcp_install::run_cli(&args[2..], bridge_path));
    }

    if is_hooks_subcommand {
        let binaries = if args.get(2).map(String::as_str) == Some("setup") {
            extract_integration_binaries("paneflow hooks")
        } else {
            None
        };
        std::process::exit(paneflow_mcp_install::run_hooks_cli(&args[2..], binaries));
    }

    if is_integrations_subcommand {
        let binaries = if args.get(2).map(String::as_str) == Some("install") {
            extract_integration_binaries("paneflow integrations")
        } else {
            None
        };
        std::process::exit(paneflow_mcp_install::run_integrations_cli(
            &args[2..],
            binaries,
        ));
    }

    if is_cli_subcommand {
        std::process::exit(cli::run());
    }

    if is_unknown_verb && let Some(verb) = args.get(1) {
        eprintln!("paneflow: unknown verb '{verb}'; see `paneflow --help` for the verb list");
        std::process::exit(2);
    }

    unsafe { runtime_paths::shed_inherited_instance_env() };

    warn_if_legacy_run_install();

    match ai_hooks::extract::ensure_bridge_extracted() {
        Ok(path) => log::info!("paneflow: MCP bridge ready at {}", path.display()),
        Err(e) => log::warn!(
            "paneflow: MCP bridge extraction failed ({e:#}); `paneflow mcp install` will be unavailable until resolved"
        ),
    }
    if let Err(error) = ai_hooks::extract::ensure_ai_hook_extracted() {
        log::warn!("paneflow: AI hook extraction failed ({error:#})");
    }
    startup_trace::mark("bridge_extracted");

    host_bootstrap::start_in_background();
    worker_bootstrap::start_in_background();

    #[cfg(target_os = "windows")]
    if let Err(err) = windows_app_identity::ensure_process_app_user_model_id() {
        log::warn!("paneflow: Windows app identity setup failed: {err}");
    }

    let _timer_resolution = app::win_timer::high_resolution_timer();

    application()
        .with_assets(assets::Assets)
        .run(|cx: &mut App| {
            startup_trace::mark("gpui_app_ready");
            let config = paneflow_config::loader::load_config();
            startup_trace::mark("config_loaded");
            cx.set_text_rendering_mode(gpui::TextRenderingMode::Grayscale);
            keybindings::apply_keybindings(cx, &config.shortcuts);

            if let Err(e) = assets::Assets.load_fonts(cx) {
                log::warn!(
                    "Assets::load_fonts failed: {e}; text rendering may fail on \
                     systems without a system monospace font"
                );
            }
            startup_trace::mark("fonts_loaded");

            #[cfg(target_os = "macos")]
            {
                install_macos_menu_bar(cx);
                install_macos_menu_action_fallbacks(cx);
            }

            let bounds = crate::window_state::initial_bounds(cx);
            let decorations = match config.window_decorations.as_deref() {
                Some("server") => WindowDecorations::Server,
                Some("client") | None => WindowDecorations::Client,
                Some(other) => {
                    log::warn!(
                        "Invalid window_decorations value '{}', using 'client'",
                        other
                    );
                    WindowDecorations::Client
                }
            };

            #[cfg_attr(target_os = "macos", allow(clippy::needless_update))]
            let titlebar_options = gpui::TitlebarOptions {
                title: None,
                appears_transparent: true,
                #[cfg(target_os = "macos")]
                traffic_light_position: Some(point(px(12.0), px(10.0))),
                ..Default::default()
            };

            startup_trace::mark("window_requested");
            let window_result = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(crate::window_state::minimum_size()),
                    window_decorations: Some(decorations),
                    titlebar: Some(titlebar_options),
                    window_background: crate::app::constants::window_background_appearance(
                        config.window_backdrop.as_deref(),
                    ),
                    app_id: Some("paneflow".into()),
                    ..Default::default()
                },
                |window, cx| {
                    #[cfg(target_os = "windows")]
                    if crate::app::constants::window_backdrop_uses_mica(
                        config.window_backdrop.as_deref(),
                    ) {
                        crate::window_chrome::backdrop::apply_wallpaper_mica(
                            window,
                            crate::theme::active_theme().background.l > 0.5,
                        );
                    }
                    #[cfg(target_os = "macos")]
                    if crate::app::constants::macos_sidebar_material_enabled(
                        config.window_backdrop.as_deref(),
                    ) {
                        crate::window_chrome::macos_backdrop::apply_subtle_sidebar_material(
                            window,
                            crate::theme::active_theme().background.l > 0.5,
                            config.macos_chrome_material_enabled(),
                        );
                    }
                    #[cfg(target_os = "linux")]
                    crate::window_chrome::linux_backdrop::apply_subtle_chrome_material(window);

                    startup_trace::mark("window_created");
                    mount_paneflow_app(window, cx)
                },
            );

            startup_trace::mark("window_open_returned");
            match window_result {
                Ok(_) => cx.activate(true),
                Err(e) => {
                    log::error!("Failed to open PaneFlow window: {e}");
                    #[cfg(target_os = "linux")]
                    eprintln!(
                        "Error: PaneFlow requires a GPU with Vulkan support.\n\n\
                         Install mesa-vulkan-drivers (AMD/Intel) or your GPU's proprietary driver.\n\n\
                         Install commands:\n\
                         \x20 Debian/Ubuntu:  sudo apt install mesa-vulkan-drivers\n\
                         \x20 Fedora/RHEL:    sudo dnf install mesa-vulkan-drivers\n\
                         \x20 Arch:           sudo pacman -S vulkan-radeon vulkan-intel or nvidia-utils\n\n\
                         Run `vulkaninfo` to verify Vulkan support.\n\
                         If drivers are already installed, run with RUST_LOG=error for details.\n\n\
                         Underlying error: {e}"
                    );
                    #[cfg(target_os = "windows")]
                    eprintln!(
                        "Error: PaneFlow could not create its GPU-backed window on Windows.\n\n\
                         Update your GPU driver from NVIDIA, AMD, Intel, or your PC vendor, then restart Paneflow.\n\
                         If this started after enabling a native backdrop, launch once with:\n\
                         \x20 PANEFLOW_WINDOW_BACKDROP=off\n\n\
                         Underlying error: {e}"
                    );
                    #[cfg(target_os = "macos")]
                    eprintln!(
                        "Error: PaneFlow could not create its GPU-backed window on macOS.\n\n\
                         Update macOS and restart Paneflow. If this started after enabling a native backdrop, launch once with:\n\
                         \x20 PANEFLOW_WINDOW_BACKDROP=off\n\n\
                         Underlying error: {e}"
                    );
                    std::process::exit(1);
                }
            }
        });
}

#[cfg(all(test, windows))]
mod windows_startup_console_tests {
    use super::should_detach_windows_console;

    #[test]
    fn gui_launch_detaches_only_a_lonely_console() {
        assert!(should_detach_windows_console(false, 1));
        assert!(!should_detach_windows_console(false, 0));
        assert!(!should_detach_windows_console(false, 2));
    }

    #[test]
    fn scriptable_invocation_keeps_console_even_when_lonely() {
        assert!(!should_detach_windows_console(true, 1));
    }
}

#[cfg(test)]
mod tests {
    use super::{should_extract_mcp_bridge_for_cli, should_load_login_shell_env_for_startup};

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_string()).collect()
    }

    #[test]
    fn login_shell_env_capture_only_runs_for_gui_launches() {
        assert!(should_load_login_shell_env_for_startup(
            false, false, false, false, false, false
        ));
        assert!(!should_load_login_shell_env_for_startup(
            false, true, false, false, false, false
        ));
        assert!(!should_load_login_shell_env_for_startup(
            false, false, true, false, false, false
        ));
        assert!(!should_load_login_shell_env_for_startup(
            false, false, false, true, false, false
        ));
        assert!(!should_load_login_shell_env_for_startup(
            false, false, false, false, true, false
        ));
        assert!(!should_load_login_shell_env_for_startup(
            false, false, false, false, false, true
        ));
    }

    #[test]
    fn mcp_bridge_extraction_only_runs_for_exact_install_command() {
        assert!(should_extract_mcp_bridge_for_cli(&args(&[
            "paneflow", "mcp", "install"
        ])));
        assert!(!should_extract_mcp_bridge_for_cli(&args(&[
            "paneflow", "mcp", "status"
        ])));
        assert!(!should_extract_mcp_bridge_for_cli(&args(&[
            "paneflow",
            "mcp",
            "uninstall"
        ])));
        assert!(!should_extract_mcp_bridge_for_cli(&args(&[
            "paneflow", "mcp", "install", "--help"
        ])));
    }
}
