#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

struct Home {
    root: tempfile::TempDir,
}

impl Home {
    fn new() -> Self {
        let home = Self {
            root: tempfile::TempDir::new().unwrap(),
        };
        std::fs::create_dir_all(home.bin()).unwrap();
        std::fs::create_dir_all(home.user_tools()).unwrap();
        write_script(
            &home.bin().join("paneflow-resume"),
            &format!(
                "printf '%s|%s\\n' \"$*\" \"$PATH\" >> '{}'\n",
                home.launches().display()
            ),
        );
        home
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    fn bin(&self) -> PathBuf {
        self.path("helpers")
    }

    fn user_tools(&self) -> PathBuf {
        self.path("user-tools")
    }

    fn launches(&self) -> PathBuf {
        self.path("launches.log")
    }

    fn seen_by_startup(&self) -> PathBuf {
        self.path("seen-by-startup")
    }

    fn startup_file(&self) -> String {
        format!(
            "printf '%s\\n' \"${{PANEFLOW_RESUME_HANDOFF-unset}}\" > '{}'\nexport PATH='{}':\"$PATH\"\n",
            self.seen_by_startup().display(),
            self.user_tools().display()
        )
    }

    fn launches_text(&self) -> String {
        std::fs::read_to_string(self.launches()).unwrap_or_default()
    }

    fn run(&self, shell: &str, handoff: Option<&str>) {
        let mut env = HashMap::new();
        let args = paneflow_launch::shell::setup_shell_integration(
            Some(&self.path("integration")),
            shell,
            &mut env,
        );
        assert!(!args.is_empty() || env.contains_key("ZDOTDIR"), "{shell}");
        let mut command = Command::new(shell);
        command
            .args(&args)
            .arg("-i")
            .env_clear()
            .env("HOME", self.root.path())
            .env("PATH", "/usr/bin:/bin")
            .env("PANEFLOW_BIN_DIR", self.bin())
            .envs(&env)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(handoff) = handoff {
            command.env("PANEFLOW_RESUME_HANDOFF", handoff);
        }
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"true\ntrue\nexit\n")
            .unwrap();
        assert!(child.wait().unwrap().success(), "{shell}");
    }
}

fn write_script(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn shell_on_this_machine(name: &str) -> Option<String> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain([PathBuf::from("/usr/bin"), PathBuf::from("/bin")])
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
        .map(|path| path.display().to_string())
}

fn assert_one_launch_after_the_startup_files(home: &Home) {
    assert_eq!(
        std::fs::read_to_string(home.seen_by_startup()).unwrap(),
        "unset\n",
        "the startup files never see the handoff variable"
    );
    let launches = home.launches_text();
    let lines: Vec<&str> = launches.lines().collect();
    assert_eq!(lines.len(), 1, "the launcher runs once: {launches}");
    let (args, path) = lines[0].split_once('|').unwrap();
    assert_eq!(args, "/tmp/resume-x.handoff");
    let entries: Vec<&str> = path.split(':').collect();
    assert_eq!(entries[0], home.bin().display().to_string());
    assert!(
        entries.contains(&home.user_tools().display().to_string().as_str()),
        "the launcher inherits the PATH a startup file changed: {path}"
    );
}

#[test]
fn bash_runs_the_launcher_once_at_its_first_prompt_after_bashrc_without_exposing_the_variable() {
    let Some(bash) = shell_on_this_machine("bash") else {
        return;
    };
    let home = Home::new();
    std::fs::write(home.path(".bashrc"), home.startup_file()).unwrap();
    home.run(&bash, Some("/tmp/resume-x.handoff"));
    assert_one_launch_after_the_startup_files(&home);
}

#[test]
fn zsh_runs_the_launcher_once_at_its_first_prompt_after_zshrc_without_exposing_the_variable() {
    let Some(zsh) = shell_on_this_machine("zsh") else {
        return;
    };
    let home = Home::new();
    let startup = home.startup_file();
    std::fs::write(home.path(".zshrc"), &startup).unwrap();
    std::fs::write(
        home.path(".zshenv"),
        format!(
            "printf '%s\\n' \"${{PANEFLOW_RESUME_HANDOFF-unset}}\" > '{}'\n",
            home.path("seen-by-zshenv").display()
        ),
    )
    .unwrap();
    home.run(&zsh, Some("/tmp/resume-x.handoff"));
    assert_eq!(
        std::fs::read_to_string(home.path("seen-by-zshenv")).unwrap(),
        "unset\n"
    );
    assert_one_launch_after_the_startup_files(&home);
}

#[test]
fn without_a_handoff_the_first_prompt_runs_nothing_new() {
    let Some(bash) = shell_on_this_machine("bash") else {
        return;
    };
    let home = Home::new();
    std::fs::write(home.path(".bashrc"), home.startup_file()).unwrap();
    home.run(&bash, None);
    assert_eq!(home.launches_text(), "");
    if let Some(zsh) = shell_on_this_machine("zsh") {
        home.run(&zsh, None);
        assert_eq!(home.launches_text(), "");
    }
}
