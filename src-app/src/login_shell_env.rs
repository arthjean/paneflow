#[cfg(not(unix))]
pub fn load_login_shell_env() {}

#[cfg(unix)]
const CAPTURE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[cfg(unix)]
const MAX_CAPTURE_BYTES: usize = 1024 * 1024;

#[cfg(unix)]
pub fn load_login_shell_env() {
    use std::os::unix::process::CommandExt as _;
    use std::process::Command;

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
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

    let capture = match capture_login_env(cmd, CAPTURE_TIMEOUT, MAX_CAPTURE_BYTES) {
        Ok(capture) => capture,
        Err(e) => {
            log::debug!("login-shell env: could not spawn {capture_shell:?}: {e}");
            return;
        }
    };
    if capture.truncated {
        log::warn!(
            "login-shell env: {capture_shell:?} wrote more than {MAX_CAPTURE_BYTES} bytes; stopped reading"
        );
    }
    if capture.timed_out {
        log::warn!(
            "login-shell env: {capture_shell:?} did not exit within {}s",
            CAPTURE_TIMEOUT.as_secs()
        );
    }

    match extract_path(&capture.output, MARKER.as_bytes()) {
        Some(path) if !path.is_empty() => {
            unsafe { std::env::set_var("PATH", &path) };
            log::info!(
                "login-shell env: adopted PATH from {capture_shell:?} ({} bytes)",
                path.len()
            );
        }
        _ => {
            log::warn!(
                "login-shell env: no PATH captured from {capture_shell:?} (unsupported shell, empty env, or no complete PATH line); keeping the inherited PATH"
            );
        }
    }
}

#[cfg(unix)]
#[derive(Default)]
struct CaptureBuffer {
    output: Vec<u8>,
    finished: bool,
    truncated: bool,
}

#[cfg(unix)]
struct LoginEnvCapture {
    output: Vec<u8>,
    timed_out: bool,
    truncated: bool,
}

#[cfg(unix)]
fn capture_login_env(
    mut cmd: std::process::Command,
    timeout: std::time::Duration,
    cap: usize,
) -> std::io::Result<LoginEnvCapture> {
    use std::io::Read;
    use std::process::Stdio;
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::{Duration, Instant};

    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd.spawn()?;
    let buffer = Arc::new(Mutex::new(CaptureBuffer::default()));
    if let Some(mut stdout) = child.stdout.take() {
        let shared = Arc::clone(&buffer);
        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            loop {
                let read = match stdout.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                let mut guard = shared.lock().unwrap_or_else(PoisonError::into_inner);
                let room = cap.saturating_sub(guard.output.len());
                guard.output.extend_from_slice(&chunk[..read.min(room)]);
                if guard.output.len() >= cap {
                    guard.truncated = true;
                    break;
                }
            }
            shared
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .finished = true;
        });
    }

    let deadline = Instant::now() + timeout;
    let timed_out = loop {
        match child.try_wait() {
            Ok(Some(_)) => break false,
            Ok(None) if Instant::now() >= deadline => break true,
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => break true,
        }
    };
    if timed_out {
        terminate_login_shell_capture(&mut child);
    }
    let _ = child.wait();

    let drain_deadline = Instant::now() + Duration::from_millis(250);
    let mut last_len = None;
    loop {
        let (len, finished) = {
            let guard = buffer.lock().unwrap_or_else(PoisonError::into_inner);
            (guard.output.len(), guard.finished)
        };
        if finished || last_len == Some(len) || Instant::now() >= drain_deadline {
            break;
        }
        last_len = Some(len);
        std::thread::sleep(Duration::from_millis(20));
    }
    let guard = buffer.lock().unwrap_or_else(PoisonError::into_inner);
    Ok(LoginEnvCapture {
        output: guard.output.clone(),
        timed_out,
        truncated: guard.truncated,
    })
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
    let region = &buf[start..];
    let complete = region.iter().rposition(|&b| b == b'\n')?;
    for line in region[..complete].split(|&b| b == b'\n') {
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
    use super::{capture_login_env, extract_path, find_subslice, is_posix_capture_shell};
    use std::process::Command;
    use std::time::{Duration, Instant};

    fn own_group(cmd: &mut Command) {
        use std::os::unix::process::CommandExt as _;
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }

    #[test]
    fn a_background_job_in_the_rc_does_not_hold_the_capture() {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg("sleep 30 & printf '%s\\n' __M__; exec env")
            .env("PATH", "/usr/bin:/bin:/paneflow-login-path");
        own_group(&mut cmd);
        let started = Instant::now();
        let capture = capture_login_env(cmd, Duration::from_secs(5), 1024 * 1024).unwrap();
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_millis(500), "took {elapsed:?}");
        assert!(!capture.timed_out);
        assert_eq!(
            extract_path(&capture.output, b"__M__").as_deref(),
            Some("/usr/bin:/bin:/paneflow-login-path")
        );
    }

    #[test]
    fn a_complete_path_line_is_adopted_when_the_shell_overstays() {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg("printf '__M__\\nPATH=/adopted\\nPARTIAL=' ; sleep 30");
        own_group(&mut cmd);
        let capture = capture_login_env(cmd, Duration::from_millis(300), 1024 * 1024).unwrap();
        assert!(capture.timed_out);
        assert_eq!(
            extract_path(&capture.output, b"__M__").as_deref(),
            Some("/adopted")
        );
    }

    #[test]
    fn endless_rc_output_stops_at_the_cap_and_keeps_the_inherited_path() {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg("yes paneflow-noise");
        own_group(&mut cmd);
        let started = Instant::now();
        let capture = capture_login_env(cmd, Duration::from_millis(500), 64 * 1024).unwrap();
        assert!(capture.truncated);
        assert_eq!(capture.output.len(), 64 * 1024);
        assert_eq!(extract_path(&capture.output, b"__M__"), None);
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn an_unterminated_path_line_is_not_adopted() {
        assert_eq!(extract_path(b"__M__\nPATH=/partial", b"__M__"), None);
    }

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
