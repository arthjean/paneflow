use std::io::{BufRead, BufReader};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

use anyhow::{Context, Result};

use super::super::error::UpdateError;
use super::super::install_method::PackageManager;

const STDERR_BUFFER_CAP_BYTES: usize = 1024 * 1024;

const PKEXEC_ABSOLUTE_PATH: &str = "/usr/bin/pkexec";

const DRAIN_CHANNEL_CAPACITY: usize = 1024;

enum DrainedLine {
    Stdout(String),
    Stderr(String),
}

pub(crate) const BUSY_MESSAGE: &str = "Package manager is busy - try again in a moment.";

const DNF_LOCK_PATH: &str = "/run/dnf/rpmtransaction.lock";

pub fn run_update(manager: &PackageManager, version: &str) -> Result<()> {
    run_update_impl(
        manager,
        version,
        which::which("pkexec").is_ok(),
        Path::new(PKEXEC_ABSOLUTE_PATH),
    )
}

fn run_update_impl(
    manager: &PackageManager,
    version: &str,
    pkexec_installed: bool,
    pkexec_spawn_path: &Path,
) -> Result<()> {
    if matches!(manager, PackageManager::Other | PackageManager::RpmOstree) {
        return Err(anyhow::Error::new(UpdateError::Other(
            "pkexec branch reached with non-dnf/apt/zypper PackageManager".into(),
        )));
    }

    let normalized_version = validate_version(version)?;

    if !pkexec_installed {
        return Err(anyhow::Error::new(UpdateError::EnvironmentBroken {
            message: "pkexec not found - update via your system package manager".into(),
        }));
    }

    let busy_context: Option<String> = match manager {
        PackageManager::Dnf => dnf_lock_held()
            .then(|| format!("dnf lock held during pre-flight ({DNF_LOCK_PATH} present)")),
        PackageManager::Apt => apt_lock_owner_from_proc(Path::new("/proc"))
            .map(|pid| format!("apt/dpkg transaction in flight during pre-flight (pid {pid})")),
        PackageManager::Zypper => None,
        PackageManager::Other | PackageManager::RpmOstree => None,
    };
    if let Some(diag) = busy_context {
        return Err(anyhow::Error::new(UpdateError::Other(BUSY_MESSAGE.into())).context(diag));
    }

    let argv = build_argv(manager, normalized_version);
    let manager_label = manager_label(manager).to_string();

    let (_display_program, args) = argv
        .split_first()
        .context("build_argv returned an empty command vector")?;

    let mut child = Command::new(pkexec_spawn_path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn `{}`", argv.join(" ")))?;

    let stdout = child
        .stdout
        .take()
        .context("pkexec child did not expose stdout")?;
    let stderr = child
        .stderr
        .take()
        .context("pkexec child did not expose stderr")?;

    let (tx, rx) = mpsc::sync_channel::<DrainedLine>(DRAIN_CHANNEL_CAPACITY);
    let tx_stderr = tx.clone();
    let stdout_handle = thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if tx.send(DrainedLine::Stdout(line)).is_err() {
                break;
            }
        }
    });
    let stderr_handle = thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines().map_while(Result::ok) {
            if tx_stderr.send(DrainedLine::Stderr(line)).is_err() {
                break;
            }
        }
    });

    let mut stderr_buf: Vec<String> = Vec::new();
    let mut stderr_bytes: usize = 0;
    let mut stderr_truncated = false;
    for event in rx {
        match event {
            DrainedLine::Stdout(line) => {
                log::info!("self-update/{manager_label}: {line}");
            }
            DrainedLine::Stderr(line) => {
                stderr_bytes = stderr_bytes.saturating_add(line.len().saturating_add(1));
                if stderr_bytes > STDERR_BUFFER_CAP_BYTES {
                    stderr_truncated = true;
                } else {
                    stderr_buf.push(line);
                }
            }
        }
    }
    if stderr_truncated {
        log::warn!("self-update: stderr buffer truncated at 1 MiB cap");
    }

    let _ = stdout_handle.join();
    let _ = stderr_handle.join();

    let status = child.wait().context("failed to wait on pkexec child")?;

    if status.success() {
        log::info!("self-update/{manager_label}: upgrade succeeded");
        return Ok(());
    }

    let code = status.code();
    let signal = status.signal();
    let update_err = classify_exit(code, signal, &stderr_buf, &manager_label);

    let wrapped = anyhow::Error::new(update_err);
    let wrapped = match (code, signal) {
        (Some(n), _) => wrapped.context(format!("pkexec exited with code {n}")),
        (None, Some(sig)) => wrapped.context(format!("pkexec killed by signal {sig}")),
        (None, None) => wrapped,
    };
    Err(wrapped)
}

fn manager_label(manager: &PackageManager) -> &'static str {
    match manager {
        PackageManager::Dnf => "dnf",
        PackageManager::Apt => "apt",
        PackageManager::Zypper => "zypper",
        PackageManager::Other => "other",
        PackageManager::RpmOstree => "rpm-ostree",
    }
}

fn validate_version(raw: &str) -> Result<&str> {
    let rest = raw.strip_prefix('v').unwrap_or(raw);

    let mut completed_parts: usize = 0;
    let mut segment_len: usize = 0;
    for ch in rest.chars() {
        match ch {
            '0'..='9' => {
                segment_len = segment_len.saturating_add(1);
            }
            '.' => {
                if segment_len == 0 {
                    return Err(invalid_version(raw));
                }
                completed_parts = completed_parts.saturating_add(1);
                segment_len = 0;
            }
            _ => return Err(invalid_version(raw)),
        }
    }
    if segment_len == 0 {
        return Err(invalid_version(raw));
    }
    completed_parts = completed_parts.saturating_add(1);
    if completed_parts != 3 {
        return Err(invalid_version(raw));
    }

    Ok(rest)
}

fn invalid_version(raw: &str) -> anyhow::Error {
    anyhow::Error::new(UpdateError::Other(format!(
        "Invalid version string: {raw:?}"
    )))
}

fn build_argv(manager: &PackageManager, version_stripped: &str) -> Vec<String> {
    match manager {
        PackageManager::Dnf => vec![
            "pkexec".into(),
            "dnf".into(),
            "--refresh".into(),
            "install".into(),
            "-y".into(),
            "--best".into(),
            "--setopt=install_weak_deps=False".into(),
            format!("paneflow-{version_stripped}"),
        ],
        PackageManager::Apt => vec![
            "pkexec".into(),
            "sh".into(),
            "-c".into(),
            "apt-get update -q && apt-get install -y --no-install-recommends \"paneflow=$1\""
                .into(),
            "_".into(),
            format!("{version_stripped}-1"),
        ],
        PackageManager::Zypper => vec![
            "pkexec".into(),
            "sh".into(),
            "-c".into(),
            "zypper --non-interactive --gpg-auto-import-keys refresh && zypper --non-interactive install --no-recommends --force \"paneflow=$1\""
                .into(),
            "_".into(),
            version_stripped.into(),
        ],
        PackageManager::Other | PackageManager::RpmOstree => Vec::new(),
    }
}

fn classify_exit(
    code: Option<i32>,
    signal: Option<i32>,
    stderr_buf: &[String],
    manager_label: &str,
) -> UpdateError {
    if let Some(sig) = signal {
        return UpdateError::Other(format!("package manager killed by signal {sig}"));
    }

    match code {
        Some(0) => {
            UpdateError::Other("classify_exit called on a successful status - caller bug".into())
        }
        Some(126) => UpdateError::InstallDeclined {
            message: "Authentication cancelled".into(),
        },
        Some(127) => UpdateError::EnvironmentBroken {
            message: "pkexec returned 127 (no polkit agent or command missing)".into(),
        },
        Some(n) if (129..=159).contains(&n) => {
            let sig = n - 128;
            UpdateError::Other(format!("package manager killed by signal {sig}"))
        }
        Some(n) => {
            if !stderr_buf.is_empty() {
                log::debug!(
                    "self-update/{manager_label}: stderr (exit {n}):\n{}",
                    stderr_buf.join("\n")
                );
            }
            UpdateError::InstallFailed {
                log_path: PathBuf::new(),
            }
        }
        None => UpdateError::Other("package manager exited without an exit code or signal".into()),
    }
}

fn dnf_lock_held() -> bool {
    dnf_lock_held_at(Path::new(DNF_LOCK_PATH))
}

fn dnf_lock_held_at(path: &Path) -> bool {
    match path.try_exists() {
        Ok(exists) => exists,
        Err(err) => {
            log::warn!(
                "self-update/dnf: lock probe at {} failed ({err}); proceeding without pre-flight",
                path.display()
            );
            false
        }
    }
}

const APT_PROCESS_COMMS: &[&str] = &[
    "apt",
    "apt-get",
    "apt.systemd.da",
    "dpkg",
    "unattended-upgr",
];

fn apt_lock_owner_from_proc(proc_root: &Path) -> Option<u32> {
    let entries = match std::fs::read_dir(proc_root) {
        Ok(e) => e,
        Err(err) => {
            log::warn!(
                "self-update/apt: /proc scan at {} failed ({err}); proceeding without pre-flight",
                proc_root.display()
            );
            return None;
        }
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if !name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };
        let comm_path = entry.path().join("comm");
        let Ok(bytes) = std::fs::read(&comm_path) else {
            continue;
        };
        let comm = match std::str::from_utf8(&bytes) {
            Ok(s) => s.trim_end_matches(['\n', '\0']),
            Err(_) => continue,
        };
        if APT_PROCESS_COMMS.contains(&comm) {
            return Some(pid);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn dnf_lock_held_returns_true_when_lock_file_present() {
        let dir = tempdir().unwrap();
        let lock = dir.path().join("rpmtransaction.lock");
        fs::write(&lock, b"").unwrap();
        assert!(dnf_lock_held_at(&lock));
    }

    #[test]
    fn dnf_lock_held_returns_false_when_lock_file_absent() {
        let dir = tempdir().unwrap();
        let lock = dir.path().join("rpmtransaction.lock");
        assert!(!dnf_lock_held_at(&lock));
    }

    fn fake_proc_entry(proc_root: &Path, pid: &str, comm: &str) {
        let dir = proc_root.join(pid);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("comm"), format!("{comm}\n")).unwrap();
    }

    #[test]
    fn apt_lock_held_detects_running_dpkg_in_proc_scan() {
        let dir = tempdir().unwrap();
        fake_proc_entry(dir.path(), "1", "systemd");
        fake_proc_entry(dir.path(), "123", "bash");
        fake_proc_entry(dir.path(), "456", "dpkg");
        assert_eq!(apt_lock_owner_from_proc(dir.path()), Some(456));
    }

    #[test]
    fn apt_lock_held_detects_unattended_upgr_truncated_comm() {
        let dir = tempdir().unwrap();
        fake_proc_entry(dir.path(), "999", "unattended-upgr");
        assert_eq!(apt_lock_owner_from_proc(dir.path()), Some(999));
    }

    #[test]
    fn apt_lock_held_returns_false_when_no_pkg_mgr_process() {
        let dir = tempdir().unwrap();
        fake_proc_entry(dir.path(), "1", "systemd");
        fake_proc_entry(dir.path(), "2", "kthreadd");
        fake_proc_entry(dir.path(), "123", "bash");
        assert_eq!(apt_lock_owner_from_proc(dir.path()), None);
    }

    #[test]
    fn apt_lock_held_ignores_non_pid_entries() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("self")).unwrap();
        fs::write(dir.path().join("version"), b"Linux").unwrap();
        fake_proc_entry(dir.path(), "123", "bash");
        assert_eq!(apt_lock_owner_from_proc(dir.path()), None);
    }

    #[test]
    fn apt_lock_held_returns_none_when_proc_root_missing() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("not-a-proc");
        assert_eq!(apt_lock_owner_from_proc(&missing), None);
    }

    #[test]
    fn apt_lock_owner_returns_pid_for_matching_process() {
        let dir = tempdir().unwrap();
        fake_proc_entry(dir.path(), "4242", "apt-get");
        assert_eq!(apt_lock_owner_from_proc(dir.path()), Some(4242));
    }

    #[test]
    fn dnf_lock_held_at_treats_err_as_not_held() {
        let dir = tempdir().unwrap();
        let not_a_dir = dir.path().join("some_file");
        fs::write(&not_a_dir, b"").unwrap();
        let impossible = not_a_dir.join("rpmtransaction.lock");
        assert!(!dnf_lock_held_at(&impossible));
    }

    #[test]
    fn validate_version_accepts_semver() {
        assert_eq!(validate_version("v0.2.3").unwrap(), "0.2.3");
        assert_eq!(validate_version("0.2.3").unwrap(), "0.2.3");
        assert_eq!(validate_version("v99.100.42").unwrap(), "99.100.42");
    }

    #[test]
    fn validate_version_rejects_malformed() {
        for bad in [
            "",
            "0.2",
            "0.2.3.4",
            "0.2.3-rc1",
            "v0.2.3 && reboot",
            "latest",
            " 0.2.3",
            "0.2.3 ",
            "v.2.3",
            ".0.2.3",
            "0..2.3",
            "v0.2.3;rm -rf /",
        ] {
            assert!(
                validate_version(bad).is_err(),
                "should have rejected {bad:?}"
            );
        }
    }

    #[test]
    fn version_validators_agree() {
        use crate::app::self_update_flow::is_strict_semver;
        for case in [
            "v0.2.3",
            "0.2.3",
            "v99.100.42",
            "",
            "0.2",
            "0.2.3.4",
            "0.2.3-rc1",
            "v0.2.3 && reboot",
            "latest",
            " 0.2.3",
            "0.2.3 ",
            "v.2.3",
            ".0.2.3",
            "0..2.3",
            "v0.2.3;rm -rf /",
        ] {
            assert_eq!(
                is_strict_semver(case),
                validate_version(case).is_ok(),
                "validators disagree on {case:?}"
            );
        }
    }

    #[test]
    fn build_dnf_argv_strips_v_prefix() {
        let normalised = validate_version("v0.2.3").unwrap();
        let argv = build_argv(&PackageManager::Dnf, normalised);
        assert!(
            argv.iter().any(|t| t == "paneflow-0.2.3"),
            "argv missing NEVRA form: {argv:?}"
        );
        assert!(
            !argv.iter().any(|t| t.contains("paneflow-v")),
            "argv still has leading v: {argv:?}"
        );
    }

    #[test]
    fn build_dnf_argv_rejects_shell_metachars_via_regex() {
        assert!(validate_version("v0.2.3; rm -rf /").is_err());
        assert!(validate_version("0.2.3|cat /etc/shadow").is_err());
        assert!(validate_version("0.2.3\n/bin/sh").is_err());
    }

    #[test]
    fn build_dnf_argv_includes_best_and_weak_deps_setopt() {
        let argv = build_argv(&PackageManager::Dnf, "0.2.3");
        assert!(argv.iter().any(|t| t == "--best"), "argv: {argv:?}");
        assert!(
            argv.iter().any(|t| t == "--setopt=install_weak_deps=False"),
            "argv: {argv:?}"
        );
    }

    #[test]
    fn build_dnf_argv_puts_refresh_before_install_subcommand() {
        let argv = build_argv(&PackageManager::Dnf, "0.2.3");
        let refresh_idx = argv
            .iter()
            .position(|t| t == "--refresh")
            .unwrap_or_else(|| panic!("argv missing --refresh: {argv:?}"));
        let install_idx = argv
            .iter()
            .position(|t| t == "install")
            .unwrap_or_else(|| panic!("argv missing install: {argv:?}"));
        assert!(
            refresh_idx < install_idx,
            "--refresh ({refresh_idx}) must come before install ({install_idx}): {argv:?}"
        );
        assert_eq!(argv[0], "pkexec");
        assert_eq!(argv[1], "dnf");
        assert_eq!(argv[2], "--refresh");
        assert_eq!(argv[3], "install");
    }

    #[test]
    fn build_apt_argv_uses_equals_version_form() {
        let argv = build_argv(&PackageManager::Apt, "0.2.3");
        let script_body = argv
            .get(3)
            .cloned()
            .unwrap_or_else(|| panic!("argv too short: {argv:?}"));
        assert!(
            script_body.contains("\"paneflow=$1\""),
            "script body missing quoted positional pin: {script_body:?}"
        );
        assert!(
            !script_body.contains("paneflow-"),
            "script body used rpm `-` pin form for apt: {script_body:?}"
        );
        assert_eq!(argv.get(5).map(String::as_str), Some("0.2.3-1"));
    }

    #[test]
    fn build_apt_argv_includes_no_install_recommends() {
        let argv = build_argv(&PackageManager::Apt, "0.2.3");
        let script_body = argv
            .get(3)
            .cloned()
            .unwrap_or_else(|| panic!("argv too short: {argv:?}"));
        assert!(
            script_body.contains("--no-install-recommends"),
            "script body missing --no-install-recommends: {script_body:?}"
        );
    }

    #[test]
    fn build_apt_argv_wraps_in_sh_c_with_positional_version() {
        let argv = build_argv(&PackageManager::Apt, "0.2.3");
        let expected = vec![
            "pkexec".to_string(),
            "sh".to_string(),
            "-c".to_string(),
            "apt-get update -q && apt-get install -y --no-install-recommends \"paneflow=$1\""
                .to_string(),
            "_".to_string(),
            "0.2.3-1".to_string(),
        ];
        assert_eq!(argv, expected, "apt argv shape drifted from PRD v1.2 spec");
    }

    #[test]
    fn build_apt_argv_passes_version_as_positional_not_interpolated() {
        let malicious = "0.2.3\"; echo pwned; #";
        let argv = build_argv(&PackageManager::Apt, malicious);

        assert_eq!(
            argv.get(5).map(String::as_str),
            Some("0.2.3\"; echo pwned; #-1"),
            "version plus Debian revision must be argv[5] verbatim: {argv:?}"
        );

        let script_body = argv
            .get(3)
            .cloned()
            .unwrap_or_else(|| panic!("argv too short: {argv:?}"));
        assert!(
            !script_body.contains("echo"),
            "script body was poisoned with version content: {script_body:?}"
        );
        assert!(
            !script_body.contains("pwned"),
            "script body was poisoned with version content: {script_body:?}"
        );
        assert!(
            !script_body.contains("0.2.3"),
            "script body must not embed ANY version substring: {script_body:?}"
        );

        assert_eq!(
            script_body,
            "apt-get update -q && apt-get install -y --no-install-recommends \"paneflow=$1\"",
            "script body must be the canonical constant string"
        );
    }

    #[test]
    fn build_zypper_argv_wraps_refresh_and_install_in_one_pkexec_prompt() {
        let argv = build_argv(&PackageManager::Zypper, "0.2.3");
        let expected = vec![
            "pkexec".to_string(),
            "sh".to_string(),
            "-c".to_string(),
            "zypper --non-interactive --gpg-auto-import-keys refresh && zypper --non-interactive install --no-recommends --force \"paneflow=$1\""
                .to_string(),
            "_".to_string(),
            "0.2.3".to_string(),
        ];
        assert_eq!(
            argv, expected,
            "zypper argv shape drifted from the single-prompt install spec"
        );
    }

    #[test]
    fn build_zypper_argv_passes_version_as_positional_not_interpolated() {
        let malicious = "0.2.3\"; echo pwned; #";
        let argv = build_argv(&PackageManager::Zypper, malicious);

        assert_eq!(
            argv.get(5).map(String::as_str),
            Some("0.2.3\"; echo pwned; #"),
            "version must be argv[5] verbatim: {argv:?}"
        );
        let script_body = argv
            .get(3)
            .cloned()
            .unwrap_or_else(|| panic!("argv too short: {argv:?}"));
        assert!(
            !script_body.contains("echo") && !script_body.contains("pwned"),
            "script body was poisoned with version content: {script_body:?}"
        );
        assert!(
            script_body.contains("\"paneflow=$1\""),
            "script body missing quoted positional pin: {script_body:?}"
        );
    }

    #[test]
    fn classify_exit_status_maps_126_to_install_declined() {
        let err = classify_exit(Some(126), None, &[], "dnf");
        assert!(
            matches!(err, UpdateError::InstallDeclined { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn classify_exit_status_maps_127_to_environment_broken() {
        let err = classify_exit(Some(127), None, &[], "dnf");
        assert!(
            matches!(err, UpdateError::EnvironmentBroken { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn classify_exit_status_maps_128_plus_n_to_other_signal() {
        for (code, sig) in [(137, 9), (143, 15), (130, 2)] {
            let err = classify_exit(Some(code), None, &[], "dnf");
            match err {
                UpdateError::Other(msg) => {
                    assert!(
                        msg.contains(&format!("signal {sig}")),
                        "code {code}: got {msg}"
                    );
                }
                other => panic!("code {code}: got {other:?}"),
            }
        }
    }

    #[test]
    fn classify_exit_status_above_159_still_install_failed() {
        for code in [160_i32, 200, 255] {
            let err = classify_exit(Some(code), None, &[], "dnf");
            assert!(
                matches!(err, UpdateError::InstallFailed { .. }),
                "code {code}: got {err:?}"
            );
        }
    }

    #[test]
    fn classify_exit_status_maps_nonzero_to_install_failed() {
        for code in [1_i32, 2, 99, 255] {
            let err = classify_exit(Some(code), None, &[], "dnf");
            match err {
                UpdateError::InstallFailed { log_path } => {
                    assert!(log_path.as_os_str().is_empty(), "code {code}: path set");
                }
                other => panic!("code {code}: got {other:?}"),
            }
        }
    }

    fn make_stub_pkexec(exit_code: i32) -> (tempfile::TempDir, PathBuf) {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let script_path = dir.path().join("pkexec");
        let script =
            format!("#!/usr/bin/env bash\nexec 0<&- 1>/dev/null 2>/dev/null\nexit {exit_code}\n");
        {
            let mut file = fs::File::create(&script_path).unwrap();
            file.write_all(script.as_bytes()).unwrap();
            file.sync_all().unwrap();
        }
        let mut perms = fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).unwrap();
        (dir, script_path)
    }

    fn retry_etxtbsy<T>(mut operation: impl FnMut() -> Result<T>) -> Result<T> {
        const ETXTBSY: i32 = 26;
        const RETRY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);
        const RETRY_POLL: std::time::Duration = std::time::Duration::from_millis(5);

        let deadline = std::time::Instant::now() + RETRY_TIMEOUT;
        loop {
            let result = operation();
            let should_retry = result.as_ref().err().is_some_and(|err| {
                err.chain().any(|cause| {
                    cause
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|io| io.raw_os_error() == Some(ETXTBSY))
                })
            });
            if !should_retry {
                return result;
            }

            let now = std::time::Instant::now();
            if now >= deadline {
                return result;
            }
            std::thread::sleep(RETRY_POLL.min(deadline.saturating_duration_since(now)));
            if std::time::Instant::now() >= deadline {
                return result;
            }
        }
    }

    fn run_update_impl_retry(
        manager: &PackageManager,
        version: &str,
        pkexec_installed: bool,
        pkexec_spawn_path: &Path,
    ) -> Result<()> {
        retry_etxtbsy(|| run_update_impl(manager, version, pkexec_installed, pkexec_spawn_path))
    }

    #[test]
    fn run_update_short_circuits_when_pkexec_missing() {
        let result = run_update_impl(
            &PackageManager::Dnf,
            "0.2.3",
            false,
            Path::new("/nonexistent/pkexec-never-spawned"),
        );
        let err = result.unwrap_err();
        match UpdateError::classify(&err) {
            UpdateError::EnvironmentBroken { .. } => {}
            other => panic!("expected EnvironmentBroken, got {other:?}"),
        }
    }

    #[test]
    fn stub_pkexec_exit_0_returns_ok() {
        let (_dir, stub) = make_stub_pkexec(0);
        let result = run_update_impl_retry(&PackageManager::Dnf, "0.2.3", true, &stub);
        assert!(result.is_ok(), "got {result:?}");
    }

    #[test]
    fn stub_pkexec_exit_126_maps_to_install_declined() {
        let (_dir, stub) = make_stub_pkexec(126);
        let err = run_update_impl_retry(&PackageManager::Dnf, "0.2.3", true, &stub).unwrap_err();
        match UpdateError::classify(&err) {
            UpdateError::InstallDeclined { .. } => {}
            other => panic!("expected InstallDeclined, got {other:?}"),
        }
    }

    #[test]
    fn stub_pkexec_exit_127_maps_to_environment_broken() {
        let (_dir, stub) = make_stub_pkexec(127);
        let err = run_update_impl_retry(&PackageManager::Dnf, "0.2.3", true, &stub).unwrap_err();
        match UpdateError::classify(&err) {
            UpdateError::EnvironmentBroken { .. } => {}
            other => panic!("expected EnvironmentBroken, got {other:?}"),
        }
    }

    #[test]
    fn stub_pkexec_exit_1_maps_to_install_failed() {
        let (_dir, stub) = make_stub_pkexec(1);
        let err = run_update_impl_retry(&PackageManager::Dnf, "0.2.3", true, &stub).unwrap_err();
        match UpdateError::classify(&err) {
            UpdateError::InstallFailed { log_path } => {
                assert!(log_path.as_os_str().is_empty());
            }
            other => panic!("expected InstallFailed, got {other:?}"),
        }
    }

    #[test]
    fn stub_pkexec_exit_42_also_maps_to_install_failed() {
        let (_dir, stub) = make_stub_pkexec(42);
        let err = run_update_impl_retry(&PackageManager::Dnf, "0.2.3", true, &stub).unwrap_err();
        assert!(matches!(
            UpdateError::classify(&err),
            UpdateError::InstallFailed { .. }
        ));
    }

    #[test]
    fn stub_pkexec_rejects_malformed_version_before_spawn() {
        let (_dir, stub) = make_stub_pkexec(0);
        let err = run_update_impl_retry(&PackageManager::Dnf, "v0.2.3; rm -rf $HOME", true, &stub)
            .unwrap_err();
        match UpdateError::classify(&err) {
            UpdateError::Other(msg) => {
                assert!(
                    msg.contains("Invalid version string"),
                    "message missing expected prefix: {msg}"
                );
            }
            other => panic!("expected Other(Invalid version string …), got {other:?}"),
        }
    }

    #[test]
    fn public_run_update_wrapper_delegates_validation() {
        let err = run_update(&PackageManager::Dnf, "not-a-version").unwrap_err();
        match UpdateError::classify(&err) {
            UpdateError::Other(msg) => {
                assert!(
                    msg.contains("Invalid version string"),
                    "public wrapper did not reject malformed version: {msg}"
                );
            }
            other => panic!("expected Other, got {other:?}"),
        }
    }
}
