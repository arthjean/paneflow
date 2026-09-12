use std::process::{Command, ExitStatus, Stdio};

pub(crate) async fn run_workspace_command(mut command: Command) -> std::io::Result<ExitStatus> {
    let description = format!(
        "binary={:?} cwd={:?} args={:?}",
        command.get_program(),
        command.get_current_dir(),
        command.get_args().collect::<Vec<_>>()
    );
    log::info!("workspace launch: {description}");
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{
            CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS,
        };
        command.creation_flags(
            CREATE_BREAKAWAY_FROM_JOB | CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS,
        );
    }
    let result = smol::unblock(move || command.spawn()).await;
    let mut child = match result {
        Ok(child) => child,
        Err(error) => {
            log::warn!(
                "workspace launch failed: {description} error={error} os_error={:?}",
                error.raw_os_error()
            );
            return Err(error);
        }
    };
    log::info!("workspace spawned: {description} pid={}", child.id());
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                log::info!(
                    "workspace process exited: {description} pid={} status={status}",
                    child.id()
                );
                return Ok(status);
            }
            Ok(None) => smol::Timer::after(std::time::Duration::from_millis(500)).await,
            Err(error) => {
                log::warn!("workspace process wait failed: {description} error={error}");
                return Err(error);
            }
        };
    }
}

#[cfg(target_os = "windows")]
pub(crate) const OPEN_URL_SUBCOMMAND: &str = "__paneflow-open-url";

pub(crate) fn open_url(url: &str) -> std::io::Result<()> {
    open_url_impl(url)
}

#[cfg(target_os = "windows")]
fn open_url_impl(url: &str) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;

    use windows_sys::Win32::System::Threading::{
        CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS,
    };

    let exe = std::env::current_exe()?;
    Command::new(exe)
        .arg(OPEN_URL_SUBCOMMAND)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_BREAKAWAY_FROM_JOB | CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS)
        .spawn()
        .map(|_| ())
}

#[cfg(not(target_os = "windows"))]
fn open_url_impl(url: &str) -> std::io::Result<()> {
    open::that(url)
}

#[cfg(target_os = "windows")]
pub(crate) fn is_open_url_helper_invocation(args: &[String]) -> bool {
    args.get(1).map(String::as_str) == Some(OPEN_URL_SUBCOMMAND)
}

#[cfg(target_os = "windows")]
pub(crate) fn run_open_url_helper_from_args(args: &[String]) -> i32 {
    let Some(url) = args.get(2) else {
        eprintln!("paneflow: missing URL for {OPEN_URL_SUBCOMMAND}");
        return 2;
    };

    match open::that(url) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("paneflow: failed to open URL {url:?}: {err}");
            1
        }
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn workspace_native_launcher_escapes_app_job() {
        launch_in_app_job("native");
    }

    #[test]
    fn workspace_batch_launcher_receives_cwd_and_reports_failure() {
        launch_in_app_job("batch");
    }

    #[test]
    fn workspace_folder_opener_receives_directory_with_spaces() {
        launch_in_app_job("folder");
    }

    fn launch_in_app_job(kind: &str) {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().join("workspace space & (é)");
        std::fs::create_dir(&cwd).unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "external_open::tests::workspace_job_parent",
                "--ignored",
            ])
            .env("PANEFLOW_TEST_WORKSPACE_CWD", &cwd)
            .env("PANEFLOW_TEST_WORKSPACE_LAUNCH_KIND", kind)
            .status()
            .unwrap();
        assert!(
            status.success(),
            "workspace launch in app job failed: {status}"
        );
    }

    #[test]
    #[ignore = "subprocess with the app's process job"]
    fn workspace_job_parent() {
        let Some(cwd) = std::env::var_os("PANEFLOW_TEST_WORKSPACE_CWD") else {
            return;
        };
        use windows_sys::Win32::System::JobObjects::{
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JobObjectBasicAccountingInformation,
            QueryInformationJobObject,
        };
        let mut limits = win32job::ExtendedLimitInfo::default();
        limits.limit_kill_on_job_close().limit_breakaway_ok();
        let job = win32job::Job::create_with_limit_info(&limits).unwrap();
        job.assign_current_process().unwrap();
        let handle = job.into_handle();
        match std::env::var("PANEFLOW_TEST_WORKSPACE_LAUNCH_KIND")
            .unwrap()
            .as_str()
        {
            "batch" => check_batch_launcher(std::path::Path::new(&cwd)),
            "folder" => check_folder_opener(std::path::Path::new(&cwd)),
            "native" => {
                let mut command = Command::new(std::env::current_exe().unwrap());
                command
                    .args([
                        "--exact",
                        "external_open::tests::workspace_job_child",
                        "--ignored",
                    ])
                    .current_dir(cwd);
                let status = smol::block_on(run_workspace_command(command)).unwrap();
                assert!(status.success(), "workspace child failed: {status}");
            }
            other => panic!("unknown launch kind: {other}"),
        }
        let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        assert_ne!(
            unsafe {
                QueryInformationJobObject(
                    handle as _,
                    JobObjectBasicAccountingInformation,
                    (&raw mut accounting).cast(),
                    std::mem::size_of_val(&accounting) as u32,
                    std::ptr::null_mut(),
                )
            },
            0
        );
        assert_eq!(
            accounting.TotalProcesses, 1,
            "external launcher inherited Paneflow's job"
        );
    }

    #[test]
    #[ignore = "native executable launched from the app's process job"]
    fn workspace_job_child() {
        let Some(cwd) = std::env::var_os("PANEFLOW_TEST_WORKSPACE_CWD") else {
            return;
        };
        assert_eq!(
            std::env::current_dir().unwrap(),
            std::path::PathBuf::from(cwd)
        );
    }

    #[test]
    fn workspace_command_reports_missing_executable() {
        let error = smol::block_on(run_workspace_command(Command::new(
            "paneflow-missing-workspace-editor-67",
        )))
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    fn check_batch_launcher(cwd: &std::path::Path) {
        let temp = tempfile::tempdir().unwrap();
        let launcher = temp.path().join("editor launcher.cmd");
        std::fs::write(&launcher, "@echo off\r\necho %~1> arg.txt\r\nexit /b 7\r\n").unwrap();
        let mut command = Command::new(launcher);
        command.current_dir(cwd).arg(".");
        let status = smol::block_on(run_workspace_command(command)).unwrap();
        assert_eq!(status.code(), Some(7));
        assert_eq!(
            std::fs::read_to_string(cwd.join("arg.txt")).unwrap().trim(),
            "."
        );
    }

    fn check_folder_opener(folder: &std::path::Path) {
        let temp = tempfile::tempdir().unwrap();
        let launcher = temp.path().join("folder opener.cmd");
        std::fs::write(folder.join("folder-marker"), "workspace contents").unwrap();
        std::fs::write(
            &launcher,
            "@echo off\r\ncopy /y \"%~1\\folder-marker\" \"%~dp0received\" >nul\r\n",
        )
        .unwrap();
        smol::block_on(crate::app::workspace_ops::open_workspace_folder(
            launcher.to_str().unwrap(),
            folder,
        ))
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(temp.path().join("received")).unwrap(),
            "workspace contents"
        );
    }

    #[test]
    fn helper_invocation_is_private_subcommand_only() {
        let args = vec![
            "paneflow".to_string(),
            OPEN_URL_SUBCOMMAND.to_string(),
            "http://localhost:5173".to_string(),
        ];
        assert!(is_open_url_helper_invocation(&args));

        let other = vec!["paneflow".to_string(), "mcp".to_string()];
        assert!(!is_open_url_helper_invocation(&other));
    }

    #[test]
    fn helper_requires_url_argument() {
        let args = vec!["paneflow".to_string(), OPEN_URL_SUBCOMMAND.to_string()];
        assert_eq!(run_open_url_helper_from_args(&args), 2);
    }
}
