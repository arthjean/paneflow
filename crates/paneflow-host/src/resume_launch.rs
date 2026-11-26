use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use paneflow_program_resume::AttemptId;
use paneflow_resume_handoff::{HANDOFF_ENV, Handoff, LAUNCHER_STEM};

pub const FISH_INIT_FLAG: &str = "--init-command";
pub const ZSH_INTEGRATION_DIR: &str = "zsh";
const HANDOFF_SUFFIX: &str = ".handoff";
const HANDOFF_PREFIX: &str = "resume-";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Environment,
    FishInitCommand,
    PowerShellCommand,
}

fn shell_key(shell: &str) -> String {
    let name = shell.rsplit(['/', '\\']).next().unwrap_or(shell);
    let lower = name.to_ascii_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_owned()
}

pub fn integration_args(shell: &str, args: &[String]) -> Vec<String> {
    let integrated = match (shell_key(shell).as_str(), args) {
        ("bash", [flag, _]) => flag == "--rcfile",
        ("fish", [flag, command]) => flag == FISH_INIT_FLAG && command.starts_with("source "),
        ("pwsh" | "powershell", [logo, keep, command, _]) => {
            logo == "-NoLogo" && keep == "-NoExit" && command == "-Command"
        }
        _ => false,
    };
    if integrated {
        args.to_vec()
    } else {
        Vec::new()
    }
}

pub fn delivery(shell: &str, args: &[String], env: &BTreeMap<String, String>) -> Option<Delivery> {
    let integrated = !integration_args(shell, args).is_empty();
    match shell_key(shell).as_str() {
        "bash" if integrated => Some(Delivery::Environment),
        "zsh" if zsh_integration(env) => Some(Delivery::Environment),
        "fish" if integrated => Some(Delivery::FishInitCommand),
        "pwsh" | "powershell" if integrated => Some(Delivery::PowerShellCommand),
        _ => None,
    }
}

fn zsh_integration(env: &BTreeMap<String, String>) -> bool {
    env.get("ZDOTDIR").is_some_and(|dir| {
        Path::new(dir).file_name() == Some(std::ffi::OsStr::new(ZSH_INTEGRATION_DIR))
    })
}

pub fn deliver(
    delivery: Delivery,
    launcher: &Path,
    handoff: &Path,
    args: &mut Vec<String>,
    env: &mut BTreeMap<String, String>,
) {
    match delivery {
        Delivery::Environment => {
            env.insert(HANDOFF_ENV.to_owned(), handoff.display().to_string());
        }
        Delivery::FishInitCommand => {
            args.push(FISH_INIT_FLAG.to_owned());
            args.push(format!(
                "set -g __paneflow_resume_launcher {}; set -g __paneflow_resume_handoff {}",
                fish_quote(&launcher.display().to_string()),
                fish_quote(&handoff.display().to_string())
            ));
        }
        Delivery::PowerShellCommand => {
            if let Some(command) = args.last_mut() {
                command.push_str(&powershell_invocation(launcher, handoff));
            }
        }
    }
}

pub fn powershell_invocation(launcher: &Path, handoff: &Path) -> String {
    format!(
        "; & {} {}",
        powershell_quote(&launcher.display().to_string()),
        powershell_quote(&handoff.display().to_string())
    )
}

pub fn fish_quote(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('\'');
    for ch in text.chars() {
        if ch == '\'' || ch == '\\' {
            quoted.push('\\');
        }
        quoted.push(ch);
    }
    quoted.push('\'');
    quoted
}

pub fn powershell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

pub fn launcher_path(helper_dir: &Path) -> PathBuf {
    helper_dir.join(crate::helpers::file_name(LAUNCHER_STEM))
}

pub fn new_attempt() -> Option<AttemptId> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).ok()?;
    Some(AttemptId::from_bytes(bytes))
}

pub fn handoff_path(session_dir: &Path, attempt: &AttemptId) -> PathBuf {
    session_dir.join(format!("{HANDOFF_PREFIX}{attempt}{HANDOFF_SUFFIX}"))
}

pub fn write_handoff(
    session_dir: &Path,
    handoff: &Handoff,
    attempt: &AttemptId,
) -> io::Result<PathBuf> {
    use std::io::Write as _;
    let path = handoff_path(session_dir, attempt);
    let mut file = create_private(&path)?;
    let written = file
        .write_all(paneflow_resume_handoff::encode(handoff).as_bytes())
        .and_then(|()| file.sync_all());
    if let Err(error) = written {
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    Ok(path)
}

#[cfg(unix)]
fn create_private(path: &Path) -> io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}

#[cfg(not(unix))]
fn create_private(path: &Path) -> io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

pub fn remove_handoffs(session_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(session_dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_str().is_some_and(is_handoff_file) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

pub fn is_handoff_file(name: &str) -> bool {
    name.strip_prefix(HANDOFF_PREFIX)
        .and_then(|rest| rest.strip_suffix(HANDOFF_SUFFIX))
        .is_some_and(paneflow_resume_handoff::is_attempt_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn only_the_integration_arguments_paneflow_builds_survive_a_restart() {
        let bash = strings(&["--rcfile", "/home/u/.paneflow/shell/bash/bashrc"]);
        assert_eq!(integration_args("/usr/bin/bash", &bash), bash);
        let fish = strings(&["--init-command", "source \"/x/fish/osc7.fish\""]);
        assert_eq!(integration_args("fish", &fish), fish);
        let pwsh = strings(&["-NoLogo", "-NoExit", "-Command", ". 'C:\\x\\osc7.ps1'"]);
        assert_eq!(
            integration_args(r"C:\Program Files\PowerShell\7\pwsh.exe", &pwsh),
            pwsh
        );
        for (shell, args) in [
            ("/bin/bash", strings(&["-c", "rm -rf /tmp/x"])),
            ("/bin/bash", strings(&["--rcfile", "/x", "-c", "evil"])),
            ("fish", strings(&["--init-command", "evil"])),
            ("/bin/zsh", strings(&["-c", "evil"])),
            ("nu", strings(&["--config", "/x"])),
        ] {
            assert!(
                integration_args(shell, &args).is_empty(),
                "{shell} {args:?}"
            );
        }
    }

    #[test]
    fn a_shell_receives_the_handoff_only_through_the_integration_it_has() {
        let none = BTreeMap::new();
        let bash = strings(&["--rcfile", "/x/bash/bashrc"]);
        assert_eq!(
            delivery("/bin/bash", &bash, &none),
            Some(Delivery::Environment)
        );
        assert_eq!(delivery("/bin/bash", &[], &none), None);
        let zdotdir = BTreeMap::from([("ZDOTDIR".to_owned(), "/x/shell/zsh".to_owned())]);
        assert_eq!(
            delivery("/bin/zsh", &[], &zdotdir),
            Some(Delivery::Environment)
        );
        let users = BTreeMap::from([("ZDOTDIR".to_owned(), "/home/u/.config/zsh-own".to_owned())]);
        assert_eq!(delivery("/bin/zsh", &[], &users), None);
        let fish = strings(&["--init-command", "source '/x/osc7.fish'"]);
        assert_eq!(
            delivery("/usr/bin/fish", &fish, &none),
            Some(Delivery::FishInitCommand)
        );
        let pwsh = strings(&["-NoLogo", "-NoExit", "-Command", ". 'x'"]);
        assert_eq!(
            delivery("pwsh.exe", &pwsh, &none),
            Some(Delivery::PowerShellCommand)
        );
        assert_eq!(delivery("cmd.exe", &[], &none), None);
        assert_eq!(delivery("/usr/bin/nu", &[], &none), None);
    }

    #[test]
    fn bash_and_zsh_get_the_path_in_their_environment_and_fish_in_a_second_init_command() {
        let launcher = Path::new("/h/bin/paneflow-resume");
        let handoff = Path::new("/h/sessions/s/resume-x.handoff");
        let mut args = strings(&["--rcfile", "/x"]);
        let mut env = BTreeMap::new();
        deliver(
            Delivery::Environment,
            launcher,
            handoff,
            &mut args,
            &mut env,
        );
        assert_eq!(args, strings(&["--rcfile", "/x"]));
        assert_eq!(
            env.get(HANDOFF_ENV).map(String::as_str),
            Some("/h/sessions/s/resume-x.handoff")
        );

        let mut args = strings(&["--init-command", "source '/x'"]);
        let mut env = BTreeMap::new();
        deliver(
            Delivery::FishInitCommand,
            Path::new("/it's/launcher"),
            handoff,
            &mut args,
            &mut env,
        );
        assert!(env.is_empty());
        assert_eq!(
            args,
            strings(&[
                "--init-command",
                "source '/x'",
                "--init-command",
                "set -g __paneflow_resume_launcher '/it\\'s/launcher'; set -g __paneflow_resume_handoff '/h/sessions/s/resume-x.handoff'",
            ])
        );
    }

    #[test]
    fn powershell_gets_the_handoff_inside_its_command_string_and_no_environment_variable() {
        let mut args = strings(&["-NoLogo", "-NoExit", "-Command", ". 'C:\\x\\osc7.ps1'"]);
        let mut env = BTreeMap::new();
        deliver(
            Delivery::PowerShellCommand,
            Path::new(r"C:\Program Files\Paneflow\bin\paneflow-resume.exe"),
            Path::new(r"C:\Users\o'neil\.paneflow\sessions\s\resume-x.handoff"),
            &mut args,
            &mut env,
        );
        assert!(env.is_empty());
        assert_eq!(
            args[3],
            r". 'C:\x\osc7.ps1'; & 'C:\Program Files\Paneflow\bin\paneflow-resume.exe' 'C:\Users\o''neil\.paneflow\sessions\s\resume-x.handoff'"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_handoff_is_created_0600_under_its_attempt_name_and_never_overwrites_one() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let attempt = new_attempt().unwrap();
        let handoff = Handoff {
            attempt: attempt.to_string(),
            cwd: "/work".to_owned(),
            cwd_source: paneflow_resume_handoff::CwdSource::Report,
            argv: strings(&["nvim", "-S", "s.vim"]),
        };
        let path = write_handoff(dir.path(), &handoff, &attempt).unwrap();
        assert!(is_handoff_file(path.file_name().unwrap().to_str().unwrap()));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(
            paneflow_resume_handoff::decode(&std::fs::read_to_string(&path).unwrap()).unwrap(),
            handoff
        );
        assert!(write_handoff(dir.path(), &handoff, &attempt).is_err());
    }
}
