#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unwrap_in_result,
        clippy::panic
    )
)]

mod app;
mod connection;
mod handlers;
mod host;
mod ui;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cef::*;
use paneflow_browser_proto::{ENDPOINT_ARG, PROFILES_ARG, Refusal};

use crate::connection::Accepted;

const LOCK_FILE_NAME: &str = "paneflow-host.lock";
const LOG_FILE_NAME: &str = "host.log";

struct Options {
    endpoint: PathBuf,
    profiles: PathBuf,
}

fn option_value(name: &str) -> Option<PathBuf> {
    let prefix = format!("{name}=");
    std::env::args_os().find_map(|argument| {
        argument
            .to_str()
            .and_then(|argument| argument.strip_prefix(&prefix))
            .map(PathBuf::from)
    })
}

fn options() -> Result<Options, String> {
    let endpoint = option_value(ENDPOINT_ARG).ok_or(format!("missing {ENDPOINT_ARG}=<path>"))?;
    let profiles = option_value(PROFILES_ARG).ok_or(format!("missing {PROFILES_ARG}=<path>"))?;
    if !profiles.is_absolute() {
        return Err(format!("{PROFILES_ARG} must be an absolute path"));
    }
    Ok(Options { endpoint, profiles })
}

fn main() -> ExitCode {
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let args = args::Args::new();
    let is_browser = args
        .as_cmd_line()
        .is_some_and(|line| line.has_switch(Some(&CefString::from("type"))) != 1);
    let mut app = app::host_app();
    let code = execute_process(
        Some(args.as_main_args()),
        Some(&mut app),
        std::ptr::null_mut(),
    );
    if !is_browser {
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }
    match browser_main(&args, &mut app) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("paneflow-browser-host: {}", failure.message);
            ExitCode::from(failure.code)
        }
    }
}

struct Failure {
    code: u8,
    message: String,
}

impl Failure {
    fn new(code: u8, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

fn browser_main(args: &args::Args, app: &mut App) -> Result<(), Failure> {
    let options = options().map_err(|message| Failure::new(64, message))?;
    let mut accepted = connection::accept_parent(&options.endpoint)
        .map_err(|message| Failure::new(65, message))?;

    if let Err(refusal) = paneflow_browser_proto::check_version(
        accepted.hello.protocol_version,
        paneflow_browser_proto::PROTOCOL_VERSION,
    ) {
        return Err(refuse(&mut accepted, 66, refusal));
    }

    let lock = match lock_profiles(&options.profiles) {
        Ok(lock) => lock,
        Err(refusal) => return Err(refuse(&mut accepted, 67, refusal)),
    };

    let settings = settings(&options.profiles);
    let initialized = initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(app),
        std::ptr::null_mut(),
    );
    if initialized != 1 {
        let refusal = Refusal::EngineFailed {
            reason: format!("CefInitialize failed with exit code {}", get_exit_code()),
        };
        return Err(refuse(&mut accepted, 68, refusal));
    }

    let idle_exit = accepted.hello.idle_exit();
    let running = connection::start(accepted).map_err(|message| Failure::new(69, message))?;
    let mut host = host::Host::new(running.out.clone(), options.profiles.clone(), idle_exit);
    host.schedule_idle();
    ui::install(host);

    run_message_loop();

    running.stop_reading();
    drop(ui::uninstall());
    shutdown();
    running.finish();
    drop(lock);
    Ok(())
}

fn refuse(accepted: &mut Accepted, code: u8, refusal: Refusal) -> Failure {
    let message = refusal.to_string();
    connection::refuse(accepted, refusal);
    Failure::new(code, message)
}

fn lock_profiles(root: &Path) -> Result<std::fs::File, Refusal> {
    let engine_failed = |error: std::io::Error| Refusal::EngineFailed {
        reason: format!(
            "cannot open the profile directory {}: {error}",
            root.display()
        ),
    };
    std::fs::create_dir_all(root).map_err(engine_failed)?;
    restrict_to_owner(root);
    let lock_path = root.join(LOCK_FILE_NAME);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(engine_failed)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(Refusal::ProfileRootLocked {
            root: root.to_path_buf(),
            lock: lock_path,
        }),
        Err(std::fs::TryLockError::Error(error)) => Err(engine_failed(error)),
    }
}

#[cfg(unix)]
fn restrict_to_owner(directory: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn restrict_to_owner(_directory: &Path) {}

fn resources_dir() -> PathBuf {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .filter(|dir| dir.join("icudtl.dat").is_file());
    beside.unwrap_or_else(|| PathBuf::from(env!("PANEFLOW_CEF_SDK_DIR")))
}

fn settings(profiles: &Path) -> Settings {
    let resources = resources_dir();
    let log_file = profiles.parent().unwrap_or(profiles).join(LOG_FILE_NAME);
    let subprocess = std::env::current_exe().unwrap_or_default();
    Settings {
        windowless_rendering_enabled: 1,
        no_sandbox: 0,
        root_cache_path: profiles.to_string_lossy().as_ref().into(),
        persist_session_cookies: 1,
        browser_subprocess_path: subprocess.to_string_lossy().as_ref().into(),
        resources_dir_path: resources.to_string_lossy().as_ref().into(),
        locales_dir_path: resources.join("locales").to_string_lossy().as_ref().into(),
        log_file: log_file.to_string_lossy().as_ref().into(),
        log_severity: LogSeverity::WARNING,
        remote_debugging_port: 0,
        ..Default::default()
    }
}
