use crate::detect::{candidate_names, detect_tool_from_stem, find_real_binary_in, WRAPPED_TOOLS};
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;

#[test]
fn detect_tool_from_stem_maps_known_stems() {
    // Every wrapped tool maps to itself - the stem IS the wire id.
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

/// US-037: a real binary on `$PATH` carries the executable bit; the walk
/// now requires it (a non-executable homonym must be skipped). Test fakes
/// must therefore be made executable to stand in for real binaries.
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
    // US-037 negative test: a non-executable file named like the tool
    // earlier in $PATH must be skipped so the real (executable) binary
    // later in $PATH is returned, mirroring execvp.
    let early = tempfile::TempDir::new().unwrap();
    let late = tempfile::TempDir::new().unwrap();
    std::fs::File::create(early.path().join("claude")).unwrap(); // 0644, no x
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

/// Windows counterpart: Claude Code ships as `claude.cmd` (a Node.js
/// wrapper) on Windows today. The walk must find that file when no
/// `.exe` exists alongside. When a `.exe` exists, it must win.
#[cfg(windows)]
#[test]
fn find_real_binary_in_locates_cmd_then_exe_on_windows() {
    let dir = tempfile::TempDir::new().unwrap();
    let cmd_path = dir.path().join("claude.cmd");
    std::fs::File::create(&cmd_path).unwrap();

    // With only .cmd present, the walk falls through to it.
    let found = find_real_binary_in("claude", vec![dir.path().to_owned()], None, None);
    assert_eq!(found.as_deref(), Some(cmd_path.as_path()));

    // With both .exe and .cmd present, .exe wins per candidate ordering.
    let exe_path = dir.path().join("claude.exe");
    std::fs::File::create(&exe_path).unwrap();
    let found = find_real_binary_in("claude", vec![dir.path().to_owned()], None, None);
    assert_eq!(
        found.as_deref(),
        Some(exe_path.as_path()),
        "native .exe must take precedence over the .cmd wrapper"
    );
}

/// US-017 (cli-hardening-followup-2026-Q3): a hardlink of the
/// shim binary planted in a DIFFERENT `$PATH` directory must be
/// detected by file identity and skipped. The previous dir-only check
/// let this through, recursively re-invoking the shim every
/// time the user typed `claude` -- a single-user fork-bomb.
#[test]
fn shim_refuses_hardlink_loop() {
    let shim_dir = tempfile::TempDir::new().unwrap();
    let attacker_dir = tempfile::TempDir::new().unwrap();
    // Stand-in for the shim binary itself.
    let real_shim = shim_dir.path().join("paneflow-shim");
    std::fs::File::create(&real_shim).unwrap();
    // The hardlink shares the inode, so this also makes `attack_link`
    // executable - required now that the walk filters on the exec bit.
    #[cfg(unix)]
    make_executable(&real_shim);
    // Hardlink it into the attacker-controlled `$PATH` dir as
    // `claude` -- the dir-canonicalize check at the head of
    // `find_real_binary_in` would NOT catch this, but the
    // file-identity comparison must.
    let attack_link = attacker_dir.path().join(&candidate_names("claude")[0]);
    std::fs::hard_link(&real_shim, &attack_link).expect("hard_link");

    // `current_exe` analog: pretend the shim binary is at `real_shim`.
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

    // Sanity: with NO self_exe (i.e. degraded mode where we can't
    // compute identity), the walk falls back to dir-only semantics
    // and DOES find the attacker file. The fix is dependent on
    // current_exe() resolving correctly -- documented degradation.
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

    // The tempdir appears as both the only PATH entry AND as the self
    // dir. The self-exclusion must skip it and yield `None` - otherwise
    // the shim would exec itself and recurse.
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
    // Simulates the production layout: PATH = [shim_dir, real_dir].
    // The shim entry is self_dir and must be skipped; the second entry
    // yields the real binary.
    let shim_dir = tempfile::TempDir::new().unwrap();
    let real_dir = tempfile::TempDir::new().unwrap();

    // Create a fake `claude` in the shim dir too - this would cause
    // infinite recursion in production if self-exclusion didn't work.
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
    // Empty dir, no matching binary anywhere on the passed "PATH".
    let found = find_real_binary_in("claude", vec![dir.path().to_owned()], None, None);
    assert!(found.is_none());
}

#[test]
fn find_real_binary_in_tolerates_nonexistent_path_entries() {
    // PATH in the wild routinely contains stale directories (old
    // Python virtualenvs, uninstalled packages, typo'd PATH edits).
    // The walker must skip them silently rather than erroring.
    let dirs = vec![
        PathBuf::from("/definitely/does/not/exist/foo"),
        PathBuf::from("/also/not/real/bar"),
    ];
    let found = find_real_binary_in("claude", dirs, None, None);
    assert!(found.is_none());
}

/// Linux-gated timing guard. Replaces the PRD's "criterion benchmark"
/// (PRD US-004 AC bullet 7) with a lightweight check that stays within
/// the 15 ms budget even with a realistic number of stale `$PATH`
/// entries. Criterion would pull ~30 dev-deps for one number; this
/// guards the same invariant at ~zero cost.
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
