use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use paneflow_resume_handoff::{CwdSource, Handoff, LAUNCHER_STEM, MAX_HANDOFF_BYTES};

pub(crate) const EXIT_NOT_FOUND: u8 = 127;
pub(crate) const EXIT_REFUSED: u8 = 126;
pub(crate) const REPORT_ARG: &str = "resume-attempt";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Failure {
    NotFound,
    NotExecutable,
    CwdMissing,
    HandoffInvalid,
}

impl Failure {
    pub(crate) const fn reason(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::NotExecutable => "not_executable",
            Self::CwdMissing => "cwd_missing",
            Self::HandoffInvalid => "handoff_invalid",
        }
    }

    const fn exit_code(self) -> u8 {
        match self {
            Self::NotFound => EXIT_NOT_FOUND,
            Self::NotExecutable | Self::CwdMissing | Self::HandoffInvalid => EXIT_REFUSED,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Resolved {
    Program(PathBuf),
    Failed(Failure),
}

pub(crate) fn is_launcher_stem(stem: &str) -> bool {
    stem == LAUNCHER_STEM
}

pub(crate) fn run(args: &[OsString]) -> ExitCode {
    let [handoff_path] = args else {
        eprintln!("{LAUNCHER_STEM}: usage: {LAUNCHER_STEM} <handoff file>");
        return ExitCode::from(EXIT_REFUSED);
    };
    let text = match take_private_file(Path::new(handoff_path)) {
        Ok(text) => text,
        Err(message) => {
            eprintln!("{LAUNCHER_STEM}: {message}");
            return ExitCode::from(EXIT_REFUSED);
        }
    };
    let handoff = match paneflow_resume_handoff::decode(&text) {
        Ok(handoff) => handoff,
        Err(invalid) => {
            eprintln!("{LAUNCHER_STEM}: {invalid}");
            if let Some(attempt) = invalid.attempt.as_deref() {
                report_failure(attempt, Failure::HandoffInvalid);
            }
            return ExitCode::from(EXIT_REFUSED);
        }
    };
    if let Err(failure) = enter_directory(&handoff) {
        report_failure(&handoff.attempt, failure);
        return ExitCode::from(failure.exit_code());
    }
    let path_var = std::env::var_os("PATH");
    let program = match resolve_program(&handoff.argv[0], path_var.as_deref()) {
        Resolved::Program(program) => program,
        Resolved::Failed(failure) => {
            match failure {
                Failure::NotFound => {
                    eprintln!("{LAUNCHER_STEM}: {}: not found", handoff.argv[0]);
                }
                _ => eprintln!("{LAUNCHER_STEM}: {}: not executable", handoff.argv[0]),
            }
            report_failure(&handoff.attempt, failure);
            return ExitCode::from(failure.exit_code());
        }
    };
    start(&handoff, &program)
}

fn enter_directory(handoff: &Handoff) -> Result<(), Failure> {
    match std::env::set_current_dir(&handoff.cwd) {
        Ok(()) => Ok(()),
        Err(_) if handoff.cwd_source == CwdSource::Report => {
            eprintln!("{LAUNCHER_STEM}: {}: no such directory", handoff.cwd);
            Err(Failure::CwdMissing)
        }
        Err(_) => Ok(()),
    }
}

#[cfg(unix)]
fn take_private_file(path: &Path) -> Result<String, String> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let own_user = unsafe { libc::geteuid() };
    if !is_private_handoff(
        metadata.is_file(),
        metadata.uid(),
        metadata.mode(),
        own_user,
    ) {
        return Err(format!(
            "{}: refused: a handoff must be a regular file of this user with mode 0600",
            path.display()
        ));
    }
    read_then_remove(file, path)
}

#[cfg(unix)]
pub(crate) fn is_private_handoff(is_file: bool, owner: u32, mode: u32, own_user: u32) -> bool {
    is_file && owner == own_user && mode & 0o777 == 0o600
}

#[cfg(not(unix))]
fn take_private_file(path: &Path) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{}: refused: not a regular file", path.display()));
    }
    read_then_remove(file, path)
}

fn read_then_remove(file: std::fs::File, path: &Path) -> Result<String, String> {
    let mut bytes = Vec::new();
    let limit = u64::try_from(MAX_HANDOFF_BYTES).unwrap_or(u64::MAX) + 1;
    let read = file.take(limit).read_to_end(&mut bytes);
    let removed = std::fs::remove_file(path);
    read.map_err(|error| format!("{}: {error}", path.display()))?;
    removed.map_err(|error| format!("{}: cannot remove it: {error}", path.display()))?;
    String::from_utf8(bytes).map_err(|_| format!("{}: not UTF-8 text", path.display()))
}

pub(crate) fn resolve_program(program: &str, path_var: Option<&OsStr>) -> Resolved {
    let named = Path::new(program);
    if named.is_absolute() {
        return match executable(named) {
            Some(true) => Resolved::Program(named.to_path_buf()),
            Some(false) => Resolved::Failed(Failure::NotExecutable),
            None => Resolved::Failed(Failure::NotFound),
        };
    }
    if program.is_empty() || program.contains(['/', '\\']) {
        return Resolved::Failed(Failure::HandoffInvalid);
    }
    let mut seen_unexecutable = false;
    for dir in path_var.map(std::env::split_paths).into_iter().flatten() {
        if dir.as_os_str().is_empty() || !dir.is_absolute() {
            continue;
        }
        for name in candidate_names(program) {
            let candidate = dir.join(&name);
            match executable(&candidate) {
                Some(true) => return Resolved::Program(candidate),
                Some(false) => seen_unexecutable = true,
                None => {}
            }
        }
    }
    Resolved::Failed(if seen_unexecutable {
        Failure::NotExecutable
    } else {
        Failure::NotFound
    })
}

#[cfg(unix)]
fn candidate_names(program: &str) -> Vec<String> {
    vec![program.to_owned()]
}

#[cfg(windows)]
fn candidate_names(program: &str) -> Vec<String> {
    if Path::new(program).extension().is_some() {
        return vec![program.to_owned()];
    }
    ["exe", "com", "cmd", "bat"]
        .iter()
        .map(|extension| format!("{program}.{extension}"))
        .collect()
}

#[cfg(unix)]
fn executable(path: &Path) -> Option<bool> {
    use std::os::unix::ffi::OsStrExt;
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let raw = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    Some(unsafe { libc::access(raw.as_ptr(), libc::X_OK) } == 0)
}

#[cfg(windows)]
fn executable(path: &Path) -> Option<bool> {
    std::fs::metadata(path)
        .ok()
        .filter(std::fs::Metadata::is_file)
        .map(|_| true)
}

#[cfg(unix)]
fn start(handoff: &Handoff, program: &Path) -> ExitCode {
    use std::os::unix::process::CommandExt;
    report_started(&handoff.attempt, std::process::id());
    let error = std::process::Command::new(program)
        .arg0(&handoff.argv[0])
        .args(&handoff.argv[1..])
        .exec();
    eprintln!("{LAUNCHER_STEM}: {}: {error}", handoff.argv[0]);
    let failure = if error.kind() == std::io::ErrorKind::NotFound {
        Failure::NotFound
    } else {
        Failure::NotExecutable
    };
    report_failure(&handoff.attempt, failure);
    ExitCode::from(failure.exit_code())
}

#[cfg(windows)]
fn start(handoff: &Handoff, program: &Path) -> ExitCode {
    let _ = ctrlc::set_handler(|| {});
    let mut child = match std::process::Command::new(program)
        .args(&handoff.argv[1..])
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            eprintln!("{LAUNCHER_STEM}: {}: {error}", handoff.argv[0]);
            report_failure(&handoff.attempt, Failure::NotExecutable);
            return ExitCode::from(EXIT_REFUSED);
        }
    };
    report_started(&handoff.attempt, child.id());
    match child.wait() {
        Ok(status) => crate::exec::exit_code_from_status(&status),
        Err(error) => {
            eprintln!("{LAUNCHER_STEM}: {}: {error}", handoff.argv[0]);
            ExitCode::from(1)
        }
    }
}

fn report_started(attempt: &str, pid: u32) {
    report(&[attempt, "started", &pid.to_string()]);
}

fn report_failure(attempt: &str, failure: Failure) {
    report(&[attempt, "failed", failure.reason()]);
}

fn report(fields: &[&str]) {
    let Some(hook) = crate::locate_sibling_hook_binary() else {
        return;
    };
    let _ = std::process::Command::new(hook)
        .arg(REPORT_ARG)
        .args(fields)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn only_a_regular_0600_file_of_this_user_is_a_private_handoff() {
        assert!(is_private_handoff(true, 1000, 0o100600, 1000));
        assert!(!is_private_handoff(true, 0, 0o100600, 1000));
        assert!(!is_private_handoff(true, 1000, 0o100640, 1000));
        assert!(!is_private_handoff(true, 1000, 0o100700, 1000));
        assert!(!is_private_handoff(false, 1000, 0o040600, 1000));
    }

    #[test]
    fn a_relative_or_empty_program_is_an_invalid_handoff_and_a_missing_one_is_not_found() {
        assert_eq!(
            resolve_program("./run.sh", None),
            Resolved::Failed(Failure::HandoffInvalid)
        );
        assert_eq!(
            resolve_program("", None),
            Resolved::Failed(Failure::HandoffInvalid)
        );
        assert_eq!(
            resolve_program("paneflow-no-such-program", None),
            Resolved::Failed(Failure::NotFound)
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_name_found_only_without_the_execute_bit_is_not_executable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let program = dir.path().join("tool");
        std::fs::write(&program, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            resolve_program("tool", Some(dir.path().as_os_str())),
            Resolved::Failed(Failure::NotExecutable)
        );
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            resolve_program("tool", Some(dir.path().as_os_str())),
            Resolved::Program(program)
        );
    }
}
