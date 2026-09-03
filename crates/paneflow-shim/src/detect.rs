use std::env;
use std::path::{Path, PathBuf};

pub(crate) fn detect_tool() -> Option<&'static str> {
    let exe = env::current_exe().ok()?;
    let stem = exe.file_stem()?.to_str()?;
    detect_tool_from_stem(stem)
}

pub(crate) const WRAPPED_TOOLS: &[&str] = &[
    "claude",
    "codex",
    "opencode",
    "pi",
    "hermes",
    "grok",
    "amp",
    "cursor-agent",
    "gemini",
    "kiro-cli",
    "agy",
    "copilot",
    "codebuddy",
    "droid",
    "qodercli",
    "openclaw",
];

pub(crate) fn detect_tool_from_stem(stem: &str) -> Option<&'static str> {
    WRAPPED_TOOLS.iter().find(|t| **t == stem).copied()
}

#[cfg(unix)]
pub(crate) fn candidate_names(tool: &str) -> Vec<String> {
    vec![tool.to_owned()]
}

#[cfg(windows)]
pub(crate) fn candidate_names(tool: &str) -> Vec<String> {
    vec![format!("{tool}.exe"), format!("{tool}.cmd")]
}

pub(crate) fn find_real_binary(tool: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    let self_exe = env::current_exe().ok();
    let self_dir = self_exe
        .as_deref()
        .and_then(|p| p.parent().map(Path::to_path_buf));

    find_real_binary_in(
        tool,
        env::split_paths(&path_var),
        self_dir.as_deref(),
        self_exe.as_deref(),
    )
}

pub(crate) fn find_real_binary_in<I>(
    tool: &str,
    path_entries: I,
    self_dir: Option<&Path>,
    self_exe: Option<&Path>,
) -> Option<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
{
    let self_canon = self_dir.and_then(|d| std::fs::canonicalize(d).ok());
    let self_identity = self_exe.and_then(file_identity);
    let candidates = candidate_names(tool);

    for dir in path_entries {
        if same_canonical_dir(&self_canon, &dir) {
            continue;
        }
        for name in &candidates {
            let candidate = dir.join(name);
            if !candidate.is_file() {
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let executable = std::fs::metadata(&candidate)
                    .map(|m| m.permissions().mode() & 0o111 != 0)
                    .unwrap_or(false);
                if !executable {
                    continue;
                }
            }
            if is_same_file_as_shim(&self_identity, &candidate) {
                eprintln!(
                    "paneflow-shim: skipping {} -- matches shim identity (hardlink loop guard)",
                    candidate.display()
                );
                continue;
            }
            return Some(candidate);
        }
    }
    None
}

pub(crate) fn file_identity(path: &Path) -> Option<same_file::Handle> {
    same_file::Handle::from_path(path).ok()
}

pub(crate) fn is_same_file_as_shim(
    self_identity: &Option<same_file::Handle>,
    candidate: &Path,
) -> bool {
    match (self_identity.as_ref(), file_identity(candidate)) {
        (Some(a), Some(b)) => a == &b,
        _ => false,
    }
}

pub(crate) fn same_canonical_dir(self_canon: &Option<PathBuf>, dir: &Path) -> bool {
    match (self_canon, std::fs::canonicalize(dir).ok()) {
        (Some(s), Some(d)) => *s == d,
        _ => false,
    }
}
