use crate::detect::{candidate_names, detect_tool_from_stem, find_real_binary_in, WRAPPED_TOOLS};
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;

#[test]
fn detect_tool_from_stem_maps_known_stems() {
    for &tool in WRAPPED_TOOLS {
        assert_eq!(detect_tool_from_stem(tool), Some(tool));
    }
    assert_eq!(detect_tool_from_stem("claude"), Some("claude"));
    assert_eq!(detect_tool_from_stem("cursor-agent"), Some("cursor-agent"));
    assert_eq!(detect_tool_from_stem("qodercli"), Some("qodercli"));
}

#[test]
fn detect_tool_from_stem_rejects_everything_else() {
    assert_eq!(detect_tool_from_stem("paneflow-shim"), None);
    assert_eq!(detect_tool_from_stem("Claude"), None, "case-sensitive");
    assert_eq!(detect_tool_from_stem("claude-code"), None);
    assert_eq!(detect_tool_from_stem("OpenCode"), None);
    assert_eq!(detect_tool_from_stem(""), None);
    assert_eq!(detect_tool_from_stem(" "), None);
}

#[cfg(unix)]
#[test]
fn candidate_names_unix_returns_bare_tool() {
    assert_eq!(candidate_names("claude"), vec!["claude".to_owned()]);
    assert_eq!(candidate_names("codex"), vec!["codex".to_owned()]);
}

#[cfg(windows)]
#[test]
fn candidate_names_windows_tries_exe_then_cmd() {
    assert_eq!(
        candidate_names("claude"),
        vec!["claude.exe".to_owned(), "claude.cmd".to_owned()],
        ".exe must be tried before .cmd so native builds win over wrappers"
    );
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

#[cfg(unix)]
#[test]
fn find_real_binary_in_locates_tempdir_binary() {
    let dir = tempfile::TempDir::new().unwrap();
    let fake = dir.path().join("claude");
    std::fs::File::create(&fake).unwrap();
    make_executable(&fake);

    let found = find_real_binary_in("claude", vec![dir.path().to_owned()], None, None);
    assert_eq!(found.as_deref(), Some(fake.as_path()));
}

#[cfg(unix)]
#[test]
fn find_real_binary_in_skips_non_executable_homonym() {
    let early = tempfile::TempDir::new().unwrap();
    let late = tempfile::TempDir::new().unwrap();
    std::fs::File::create(early.path().join("claude")).unwrap();
    let real = late.path().join("claude");
    std::fs::File::create(&real).unwrap();
    make_executable(&real);

    let found = find_real_binary_in(
        "claude",
        vec![early.path().to_owned(), late.path().to_owned()],
        None,
        None,
    );
    assert_eq!(
        found.as_deref(),
        Some(real.as_path()),
        "non-executable homonym must be skipped for the executable one"
    );
}

#[cfg(windows)]
#[test]
fn find_real_binary_in_locates_cmd_then_exe_on_windows() {
    let dir = tempfile::TempDir::new().unwrap();
    let cmd_path = dir.path().join("claude.cmd");
    std::fs::File::create(&cmd_path).unwrap();

    let found = find_real_binary_in("claude", vec![dir.path().to_owned()], None, None);
    assert_eq!(found.as_deref(), Some(cmd_path.as_path()));

    let exe_path = dir.path().join("claude.exe");
    std::fs::File::create(&exe_path).unwrap();
    let found = find_real_binary_in("claude", vec![dir.path().to_owned()], None, None);
    assert_eq!(
        found.as_deref(),
        Some(exe_path.as_path()),
        "native .exe must take precedence over the .cmd wrapper"
    );
}

#[test]
fn shim_refuses_hardlink_loop() {
    let shim_dir = tempfile::TempDir::new().unwrap();
    let attacker_dir = tempfile::TempDir::new().unwrap();
    let real_shim = shim_dir.path().join("paneflow-shim");
    std::fs::File::create(&real_shim).unwrap();
    #[cfg(unix)]
    make_executable(&real_shim);
    let attack_link = attacker_dir.path().join(&candidate_names("claude")[0]);
    std::fs::hard_link(&real_shim, &attack_link).expect("hard_link");

    let found = find_real_binary_in(
        "claude",
        vec![attacker_dir.path().to_owned()],
        Some(shim_dir.path()),
        Some(real_shim.as_path()),
    );
    assert!(
        found.is_none(),
        "hardlinked shim must be skipped; got {found:?}"
    );

    let found = find_real_binary_in(
        "claude",
        vec![attacker_dir.path().to_owned()],
        Some(shim_dir.path()),
        None,
    );
    assert!(found.is_some(), "no-identity fallback finds candidate");
}

#[cfg(unix)]
#[test]
fn find_real_binary_in_excludes_self_dir() {
    let dir = tempfile::TempDir::new().unwrap();
    let fake = dir.path().join("claude");
    std::fs::File::create(&fake).unwrap();

    let found = find_real_binary_in(
        "claude",
        vec![dir.path().to_owned()],
        Some(dir.path()),
        None,
    );
    assert!(found.is_none(), "self_dir must be excluded from PATH walk");
}

#[cfg(unix)]
#[test]
fn find_real_binary_in_walks_past_self_dir_to_find_real_binary() {
    let shim_dir = tempfile::TempDir::new().unwrap();
    let real_dir = tempfile::TempDir::new().unwrap();

    std::fs::File::create(shim_dir.path().join("claude")).unwrap();
    let real_fake = real_dir.path().join("claude");
    std::fs::File::create(&real_fake).unwrap();
    make_executable(&real_fake);

    let found = find_real_binary_in(
        "claude",
        vec![shim_dir.path().to_owned(), real_dir.path().to_owned()],
        Some(shim_dir.path()),
        None,
    );
    assert_eq!(found.as_deref(), Some(real_fake.as_path()));
}

#[test]
fn find_real_binary_in_returns_none_when_absent() {
    let dir = tempfile::TempDir::new().unwrap();
    let found = find_real_binary_in("claude", vec![dir.path().to_owned()], None, None);
    assert!(found.is_none());
}

#[test]
fn find_real_binary_in_tolerates_nonexistent_path_entries() {
    let dirs = vec![
        PathBuf::from("/definitely/does/not/exist/foo"),
        PathBuf::from("/also/not/real/bar"),
    ];
    let found = find_real_binary_in("claude", dirs, None, None);
    assert!(found.is_none());
}

#[cfg(target_os = "linux")]
#[test]
fn find_real_binary_in_completes_under_15ms_budget() {
    let dirs: Vec<PathBuf> = (0..20)
        .map(|i| PathBuf::from(format!("/tmp/paneflow-nonexistent-{i}")))
        .collect();

    let start = std::time::Instant::now();
    let _ = find_real_binary_in("claude", dirs, None, None);
    let elapsed = start.elapsed();

    assert!(
        elapsed < std::time::Duration::from_millis(15),
        "PATH walk must complete under 15 ms; got {elapsed:?}"
    );
}
