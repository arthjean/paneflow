#[cfg(not(unix))]
pub fn load_login_shell_env() {}

#[cfg(unix)]
pub fn load_login_shell_env() {
    use std::io::Read;
    use std::os::unix::process::CommandExt as _;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    if unsafe { libc::isatty(libc::STDIN_FILENO) } == 1 {
        return;
    }

    let user_shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let capture_shell = if is_posix_capture_shell(&user_shell) {
        user_shell.clone()
    } else {
        "/bin/sh".to_string()
    };

    const MARKER: &str = "__PANEFLOW_LOGIN_ENV_V2__";
    let script = format!("printf '%s\\n' '{MARKER}'; exec env");

    let mut cmd = Command::new(&capture_shell);
    cmd.arg("-l").arg("-i").arg("-c").arg(&script);
    if let Some(home) = dirs::home_dir() {
        cmd.current_dir(home);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            log::debug!("login-shell env: could not spawn {capture_shell:?}: {e}");
            return;
        }
    };

    let Some(mut stdout) = child.stdout.take() else {
        terminate_login_shell_capture(&mut child);
        let _ = child.wait();
        return;
    };

    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });

    let buf = match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(buf) => {
            let _ = child.wait();
            let _ = reader.join();
            buf
        }
        Err(_) => {
            log::warn!(
                "login-shell env: {capture_shell:?} did not finish within 5s; keeping the inherited PATH"
            );
            terminate_login_shell_capture(&mut child);
            let _ = child.wait();
            if rx.recv_timeout(Duration::from_millis(250)).is_ok() {
                let _ = reader.join();
            } else {
                log::warn!(
                    "login-shell env: stdout reader stayed blocked after timeout; continuing startup"
                );
            }
            return;
        }
    };

    match extract_path(&buf, MARKER.as_bytes()) {
        Some(path) if !path.is_empty() => {
            unsafe { std::env::set_var("PATH", &path) };
            log::info!(
                "login-shell env: adopted PATH from {capture_shell:?} ({} bytes)",
                path.len()
            );
        }
        _ => {
            log::warn!(
                "login-shell env: no PATH captured from {capture_shell:?} (unsupported shell or empty env); keeping the inherited PATH"
            );
        }
    }
}

#[cfg(unix)]
fn is_posix_capture_shell(shell: &str) -> bool {
    let base = std::path::Path::new(shell)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(shell);
    matches!(
        base,
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "ash" | "mksh" | "fish"
    )
}

#[cfg(unix)]
fn terminate_login_shell_capture(child: &mut std::process::Child) {
    let child_pid = child.id();
    if child_pid <= i32::MAX as u32 {
        let pgid = child_pid as libc::pid_t;
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
}

#[cfg(unix)]
fn extract_path(buf: &[u8], marker: &[u8]) -> Option<String> {
    let start = find_subslice(buf, marker)? + marker.len();
    for line in buf[start..].split(|&b| b == b'\n') {
        if let Some(rest) = line.strip_prefix(b"PATH=") {
            return std::str::from_utf8(rest).ok().map(str::to_string);
        }
    }
    None
}

#[cfg(unix)]
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(all(test, unix))]
mod tests {
    use super::{extract_path, find_subslice, is_posix_capture_shell};

    #[test]
    fn find_subslice_locates_marker() {
        assert_eq!(find_subslice(b"junk__MARK__data", b"__MARK__"), Some(4));
        assert_eq!(find_subslice(b"__MARK__data", b"__MARK__"), Some(0));
        assert_eq!(find_subslice(b"no marker here", b"__MARK__"), None);
        assert_eq!(find_subslice(b"", b"__MARK__"), None);
        assert_eq!(find_subslice(b"data", b""), None);
    }

    #[test]
    fn extract_path_reads_path_line_after_marker() {
        let out = b"chatter\n__M__\nFOO=bar\nPATH=/a:/b:/c\nHOME=/h\n";
        assert_eq!(extract_path(out, b"__M__").as_deref(), Some("/a:/b:/c"));
        assert_eq!(extract_path(b"PATH=/x", b"__M__"), None);
        assert_eq!(extract_path(b"__M__\nFOO=bar\n", b"__M__"), None);
    }

    #[test]
    fn extract_path_survives_multiline_var_before_path() {
        let out = b"__M__\nSCRIPT=line1\nline2\nPATH=/usr/bin\n";
        assert_eq!(extract_path(out, b"__M__").as_deref(), Some("/usr/bin"));
    }

    #[test]
    fn is_posix_capture_shell_classifies() {
        for s in [
            "/bin/bash",
            "/usr/bin/zsh",
            "/bin/sh",
            "/usr/bin/fish",
            "dash",
        ] {
            assert!(is_posix_capture_shell(s), "{s} should be capturable");
        }
        for s in ["/usr/bin/nu", "/bin/tcsh", "/usr/bin/xonsh", "elvish"] {
            assert!(
                !is_posix_capture_shell(s),
                "{s} should fall back to /bin/sh"
            );
        }
    }
}
