#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const SHIM: &str = env!("CARGO_BIN_EXE_paneflow-shim");
const EXE: &str = if cfg!(windows) { ".exe" } else { "" };

fn install_shim(root: &Path, with_hook: bool) -> PathBuf {
    let dir = if with_hook {
        root.join("cache").join("bin").join("0.17.5")
    } else {
        root.to_path_buf()
    };
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(SHIM, dir.join(format!("claude{EXE}"))).unwrap();
    if with_hook {
        std::fs::copy(SHIM, dir.join(format!("paneflow-ai-hook{EXE}"))).unwrap();
    }
    dir
}

fn shim_in(dir: &Path) -> PathBuf {
    dir.join(format!("claude{EXE}"))
}

fn run(shim: &Path, path: &[&Path]) -> (Option<i32>, String, Duration) {
    let started = Instant::now();
    let output = Command::new(shim)
        .arg("--version")
        .env("PATH", std::env::join_paths(path).unwrap())
        .env_remove("PANEFLOW_SHIM_TARGET")
        .env_remove("PANEFLOW_SESSION_ID")
        .output()
        .unwrap();
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        started.elapsed(),
    )
}

#[test]
fn two_helper_dirs_without_a_real_binary_fail_with_127_and_no_loop() {
    let nested_root = tempfile::TempDir::new().unwrap();
    let release_root = tempfile::TempDir::new().unwrap();
    let nested = install_shim(nested_root.path(), true);
    let release = install_shim(release_root.path(), true);

    let (code, stderr, elapsed) = run(&shim_in(&nested), &[&nested, &release]);

    assert_eq!(code, Some(127), "{stderr}");
    assert!(stderr.contains("'claude'"), "{stderr}");
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
}

#[test]
fn a_shim_launched_by_another_shim_refuses_to_run() {
    let first_root = tempfile::TempDir::new().unwrap();
    let second_root = tempfile::TempDir::new().unwrap();
    let first = install_shim(first_root.path(), false);
    let second = install_shim(second_root.path(), false);

    let (code, stderr, elapsed) = run(&shim_in(&first), &[&first, &second]);

    assert_eq!(code, Some(127), "{stderr}");
    assert!(
        stderr.contains("another Paneflow shim resolved this shim"),
        "{stderr}"
    );
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
}

#[cfg(unix)]
#[test]
fn a_real_binary_behind_two_helper_dirs_is_the_one_that_runs() {
    use std::os::unix::fs::PermissionsExt;
    let nested_root = tempfile::TempDir::new().unwrap();
    let release_root = tempfile::TempDir::new().unwrap();
    let real = tempfile::TempDir::new().unwrap();
    let nested = install_shim(nested_root.path(), true);
    let release = install_shim(release_root.path(), true);
    let script = real.path().join("claude");
    std::fs::write(&script, "#!/bin/sh\necho real-claude \"$@\"\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let output = Command::new(shim_in(&nested))
        .arg("--version")
        .env(
            "PATH",
            std::env::join_paths([
                nested.as_path(),
                release.as_path(),
                real.path(),
                Path::new("/bin"),
            ])
            .unwrap(),
        )
        .env_remove("PANEFLOW_SHIM_TARGET")
        .env_remove("PANEFLOW_SESSION_ID")
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "real-claude --version"
    );
}
