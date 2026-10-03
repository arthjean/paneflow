pub(crate) mod command;
pub(crate) mod teams;

use std::path::{Path, PathBuf};

use paneflow_ipc_client::{IpcClient, IpcTransport};

pub(crate) const TEAM_ENV: &str = "PANEFLOW_TMUX_TEAM";
pub(crate) const METHOD: &str = "tmux.compat";
const SHIM_NAME: &str = "tmux";
const COMPAT_DIR: &str = "tmux-compat";
const TMUX_VALUE: &str = "paneflow-tmux-compat,0,0";
const TRUECOLOR_ENV: &str = "CLAUDE_CODE_TMUX_TRUECOLOR";

pub(crate) fn is_shim_invocation(argv0: Option<&std::ffi::OsStr>) -> bool {
    argv0
        .map(Path::new)
        .and_then(Path::file_stem)
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|stem| stem.eq_ignore_ascii_case(SHIM_NAME))
}

pub(crate) fn compat_dir_in(home: &Path) -> PathBuf {
    home.join("bin").join(COMPAT_DIR)
}

pub(crate) fn compat_dir() -> Option<PathBuf> {
    paneflow_home::paneflow_home().map(|home| compat_dir_in(&home))
}

#[cfg(unix)]
pub(crate) fn ensure_shim_link_in(home: &Path, exe: &Path) -> std::io::Result<PathBuf> {
    let dir = compat_dir_in(home);
    std::fs::create_dir_all(&dir)?;
    let link = dir.join(SHIM_NAME);
    if std::fs::read_link(&link).is_ok_and(|current| current == exe) {
        return Ok(link);
    }
    let staged = dir.join(format!(".{SHIM_NAME}.{}", std::process::id()));
    let _ = std::fs::remove_file(&staged);
    std::os::unix::fs::symlink(exe, &staged)?;
    std::fs::rename(&staged, &link)?;
    Ok(link)
}

#[cfg(unix)]
pub(crate) fn ensure_shim_link() {
    let Some(home) = paneflow_home::paneflow_home() else {
        return;
    };
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            log::warn!("paneflow: tmux-compat link skipped, current exe unknown ({error})");
            return;
        }
    };
    if let Err(error) = ensure_shim_link_in(&home, &exe) {
        log::warn!(
            "paneflow: tmux-compat link in {} failed ({error}); Claude Code teams cannot open panes",
            compat_dir_in(&home).display()
        );
    }
}

fn double_quotable(text: &str) -> Option<&str> {
    (!text.contains(['"', '$', '`', '\\', '\n', '\r'])).then_some(text)
}

pub(crate) fn env_prefix(dir: &Path, pane: u32) -> Result<String, String> {
    let dir = dir.to_string_lossy();
    let dir = double_quotable(&dir).ok_or_else(|| {
        format!("the Paneflow home path {dir} has characters a shell would expand")
    })?;
    Ok(format!(
        "{TRUECOLOR_ENV}=1 TMUX={TMUX_VALUE} TMUX_PANE=%{pane} PATH=\"{dir}:$PATH\""
    ))
}

pub(crate) fn teammate_command(dir: &Path, pane: u32, command: &str) -> Result<String, String> {
    let quoted = command.replace('\'', "'\\''");
    Ok(format!("{} sh -c '{quoted}'", env_prefix(dir, pane)?))
}

pub(crate) fn run_shim(argv: &[String]) -> i32 {
    if command::is_version_request(argv) {
        println!("{}", command::VERSION_LINE);
        return 0;
    }
    match call_desktop(argv) {
        Ok(reply) => {
            let stdout = reply.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
            let stderr = reply.get("stderr").and_then(|v| v.as_str()).unwrap_or("");
            if !stdout.is_empty() {
                println!("{stdout}");
            }
            if !stderr.is_empty() {
                eprintln!("{stderr}");
            }
            reply
                .get("exit")
                .and_then(serde_json::Value::as_i64)
                .and_then(|code| i32::try_from(code).ok())
                .unwrap_or(1)
        }
        Err(message) => {
            eprintln!("paneflow tmux-compat: {message}");
            1
        }
    }
}

fn call_desktop(argv: &[String]) -> Result<serde_json::Value, String> {
    let team = std::env::var(TEAM_ENV)
        .ok()
        .filter(|token| !token.is_empty())
        .ok_or_else(|| format!("{TEAM_ENV} is not set; this pane is not a Paneflow team pane"))?;
    let surface_id: u64 = std::env::var("PANEFLOW_SURFACE_ID")
        .ok()
        .and_then(|value| value.parse().ok())
        .ok_or("PANEFLOW_SURFACE_ID is not set; run inside a Paneflow pane")?;
    let endpoint = paneflow_home::ipc_endpoint().ok_or("cannot locate the Paneflow IPC socket")?;
    let mut params = serde_json::json!({
        "team": team,
        "surface_id": surface_id,
        "argv": argv,
    });
    if let Ok(session) = std::env::var("PANEFLOW_SESSION_ID")
        && !session.is_empty()
    {
        params["scope_session"] = serde_json::Value::String(session);
    }
    IpcClient::new(endpoint.path).call(METHOD, params)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_desktop_started_from_a_team_pane_sheds_the_team_token() {
        assert!(paneflow_host::env::PANE_CONTEXT_ENV.contains(&TEAM_ENV));
    }

    #[test]
    fn only_a_binary_named_tmux_is_the_shim() {
        assert!(is_shim_invocation(Some("tmux".as_ref())));
        assert!(is_shim_invocation(Some(
            "/home/u/.paneflow/bin/tmux-compat/tmux".as_ref()
        )));
        assert!(is_shim_invocation(Some("/opt/TMUX".as_ref())));
        assert!(!is_shim_invocation(Some("/usr/bin/paneflow".as_ref())));
        assert!(!is_shim_invocation(None));
    }

    #[test]
    fn the_env_prefix_puts_the_compat_dir_first_for_the_command_only() {
        let prefix = env_prefix(Path::new("/home/u/.paneflow/bin/tmux-compat"), 0).expect("ok");
        assert_eq!(
            prefix,
            "CLAUDE_CODE_TMUX_TRUECOLOR=1 TMUX=paneflow-tmux-compat,0,0 TMUX_PANE=%0 PATH=\"/home/u/.paneflow/bin/tmux-compat:$PATH\""
        );
        assert!(env_prefix(Path::new("/home/$x"), 0).is_err());
    }

    #[test]
    fn a_teammate_command_runs_through_sh_with_its_quotes_kept() {
        let command = teammate_command(
            Path::new("/h/bin/tmux-compat"),
            2,
            "cd '/w' && env A=1 claude",
        )
        .expect("ok");
        assert_eq!(
            command,
            "CLAUDE_CODE_TMUX_TRUECOLOR=1 TMUX=paneflow-tmux-compat,0,0 TMUX_PANE=%2 PATH=\"/h/bin/tmux-compat:$PATH\" sh -c 'cd '\\''/w'\\'' && env A=1 claude'"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_shim_link_points_at_the_running_binary_and_is_idempotent() {
        let home = tempfile::tempdir().expect("tempdir");
        let exe = home.path().join("paneflow");
        std::fs::write(&exe, b"").expect("fake exe");
        let link = ensure_shim_link_in(home.path(), &exe).expect("link");
        assert_eq!(link, home.path().join("bin/tmux-compat/tmux"));
        assert_eq!(std::fs::read_link(&link).expect("read link"), exe);
        let again = ensure_shim_link_in(home.path(), &exe).expect("relink");
        assert_eq!(again, link);
        let other = home.path().join("paneflow-2");
        ensure_shim_link_in(home.path(), &other).expect("retarget");
        assert_eq!(std::fs::read_link(&link).expect("read link"), other);
    }

    #[cfg(unix)]
    #[test]
    fn a_real_tmux_in_an_ordinary_pane_is_untouched() {
        let root = tempfile::tempdir().expect("tempdir");
        let real = root.path().join("real");
        let compat = root.path().join("compat");
        for dir in [&real, &compat] {
            std::fs::create_dir_all(dir).expect("dir");
            let tool = dir.join("tmux");
            std::fs::write(&tool, "#!/bin/sh\n").expect("tool");
            let mut permissions = std::fs::metadata(&tool).expect("meta").permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
            std::fs::set_permissions(&tool, permissions).expect("chmod");
        }
        let resolve = |script: String| {
            let output = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(format!("PATH={}:/usr/bin:/bin; {script}", real.display()))
                .output()
                .expect("sh runs");
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        let team = teammate_command(&compat, 1, "command -v tmux").expect("prefix");
        assert_eq!(resolve(team), compat.join("tmux").display().to_string());
        assert_eq!(
            resolve("command -v tmux".to_string()),
            real.join("tmux").display().to_string()
        );
        let after_team = format!(
            "{} true; command -v tmux",
            env_prefix(&compat, 1).expect("prefix")
        );
        assert_eq!(
            resolve(after_team),
            real.join("tmux").display().to_string(),
            "the team prefix never leaks into the pane's shell"
        );
        let ordinary = crate::agent_launcher::AgentLaunch::Builtin(
            crate::agent_launcher::TerminalAgent::ClaudeCode,
        );
        let config = paneflow_config::schema::PaneFlowConfig::default();
        let command = ordinary.launch_command(&config);
        assert!(!command.contains(COMPAT_DIR), "{command}");
        assert!(!command.contains("TMUX"), "{command}");
        assert!(ordinary.process_env().is_none());
    }
}
