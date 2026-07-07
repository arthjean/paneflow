//! External URL opening helpers.
//!
//! Windows keeps Paneflow and its children inside a kill-on-close Job Object so
//! agent CLIs and PTYs cannot outlive the app. Browser launches are the explicit
//! exception: they are user-owned once opened and must survive Paneflow exit.

#[cfg(target_os = "windows")]
use std::process::{Command, Stdio};

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
