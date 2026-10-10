use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

static INTEGRATION_WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

const ZSH_OSC7: &str = r#"# PaneFlow shell integration - OSC 7 CWD reporting
if [[ -n "${PANEFLOW_ORIG_ZDOTDIR+x}" ]]; then
    ZDOTDIR="${PANEFLOW_ORIG_ZDOTDIR}"
    unset PANEFLOW_ORIG_ZDOTDIR
else
    unset ZDOTDIR
fi
[[ -f "${ZDOTDIR:-$HOME}/.zshenv" ]] && source "${ZDOTDIR:-$HOME}/.zshenv"
__paneflow_urlencode() (
    LC_ALL=C
    str="$1"
    while [ -n "$str" ]; do
        safe="${str%%[!a-zA-Z0-9/._~-]*}"
        printf '%s' "$safe"
        str="${str#"$safe"}"
        if [ -n "$str" ]; then
            printf '%%%02X' "$(( $(printf '%d' "'$str") & 255 ))"
            str="${str#?}"
        fi
    done
)
__paneflow_osc7() { printf '\e]7;file://%s%s\a' "${HOST}" "$(__paneflow_urlencode "${PWD}")"; }
__paneflow_path_prepend() {
    [[ -z "${PANEFLOW_BIN_DIR-}" ]] && return
    # Strip every existing occurrence then prepend, keeping our dir first
    # regardless of what `.zshrc`/`.zprofile` did. Uses zsh's `path` tied
    # array so the change propagates to `$PATH` automatically.
    path=("${PANEFLOW_BIN_DIR}" "${(@)path:#${PANEFLOW_BIN_DIR}}")
}
autoload -Uz add-zsh-hook
if [[ -o interactive ]]; then
    __paneflow_osc133_precmd() {
        printf '\e]133;A\a'
    }
    __paneflow_osc133_preexec() {
        printf '\e]133;C\a'
    }
    add-zsh-hook precmd __paneflow_osc133_precmd
    add-zsh-hook preexec __paneflow_osc133_preexec
fi
add-zsh-hook chpwd __paneflow_osc7
add-zsh-hook precmd __paneflow_path_prepend
__paneflow_osc7
__paneflow_path_prepend
"#;

const BASH_OSC7: &str = r#"# PaneFlow shell integration - OSC 7 CWD reporting
[[ -f ~/.bashrc ]] && source ~/.bashrc
__paneflow_urlencode() (
    LC_ALL=C
    str="$1"
    while [ -n "$str" ]; do
        safe="${str%%[!a-zA-Z0-9/._~-]*}"
        printf '%s' "$safe"
        str="${str#"$safe"}"
        if [ -n "$str" ]; then
            printf '%%%02X' "$(( $(printf '%d' "'$str") & 255 ))"
            str="${str#?}"
        fi
    done
)
__paneflow_osc7() { printf '\e]7;file://%s%s\a' "${HOSTNAME}" "$(__paneflow_urlencode "${PWD}")"; }
__paneflow_path_prepend() {
    [[ -z "${PANEFLOW_BIN_DIR-}" ]] && return
    local p=":${PATH}:"
    p="${p//:${PANEFLOW_BIN_DIR}:/:}"
    p="${p#:}"; p="${p%:}"
    PATH="${PANEFLOW_BIN_DIR}:${p}"
    export PATH
}
__paneflow_osc133_precmd() {
    printf '\e]133;A\a'
}
PS0=$'\e]133;C\a'"${PS0-}"
PROMPT_COMMAND="__paneflow_osc133_precmd;__paneflow_osc7;__paneflow_path_prepend${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
__paneflow_path_prepend
"#;

const FISH_OSC7: &str = r#"# PaneFlow shell integration - OSC 7 CWD reporting
function __paneflow_osc7 --on-variable PWD
    printf '\e]7;file://%s%s\a' (hostname) (string escape --style=url -- "$PWD")
end
__paneflow_osc7
if set -q PANEFLOW_BIN_DIR; and test -n "$PANEFLOW_BIN_DIR"
    fish_add_path -gp $PANEFLOW_BIN_DIR
end
if status is-interactive
    function __paneflow_osc133_prompt --on-event fish_prompt
        printf '\e]133;A\a'
    end
    function __paneflow_osc133_preexec --on-event fish_preexec
        printf '\e]133;C\a'
    end
end
"#;

const WSL_SHELL_BOOTSTRAP: &str = r#"uid="$(id -u 2>/dev/null)" || uid=
shell=
if [ -n "$uid" ] && command -v getent >/dev/null 2>&1; then
    shell="$(getent passwd "$uid" 2>/dev/null | cut -d: -f7)"
fi
[ -n "$shell" ] || shell="${SHELL:-/bin/sh}"
[ -x "$shell" ] || shell=/bin/sh

case "${shell##*/}" in
    bash)
        rcfile="$(wslpath -u -- "$1" 2>/dev/null)" || exec "$shell"
        exec "$shell" --rcfile "$rcfile"
        ;;
    zsh)
        zdotdir="$(wslpath -u -- "$2" 2>/dev/null)" || exec "$shell"
        if [ "${ZDOTDIR+x}" = x ]; then
            export PANEFLOW_ORIG_ZDOTDIR="$ZDOTDIR"
        else
            unset PANEFLOW_ORIG_ZDOTDIR
        fi
        export ZDOTDIR="$zdotdir"
        exec "$shell"
        ;;
    fish)
        initfile="$(wslpath -u -- "$3" 2>/dev/null)" || exec "$shell"
        export PANEFLOW_WSL_FISH_INIT="$initfile"
        exec "$shell" --init-command 'source "$PANEFLOW_WSL_FISH_INIT"'
        ;;
    *)
        exec "$shell"
        ;;
esac
"#;

const PWSH_OSC7: &str = r#"# PaneFlow shell integration - OSC 7 CWD reporting (US-012)
# Non-destructive: wraps the existing `prompt` function so the user's
# prompt still renders. Loaded via `pwsh -NoExit -Command ". <this>"`.
# Dot-sourcing happens AFTER $PROFILE, so any user PATH mutations there
# have already run -- a one-shot prepend is sufficient. The `prompt`
# wrapper additionally re-asserts the prepend on every prompt for users
# who modify $env:PATH at runtime.

function global:__paneflow_path_prepend {
    if ([string]::IsNullOrEmpty($env:PANEFLOW_BIN_DIR)) { return }
    $sep = [System.IO.Path]::PathSeparator
    $entries = $env:PATH -split [regex]::Escape($sep) | Where-Object { $_ -ne $env:PANEFLOW_BIN_DIR }
    $env:PATH = (@($env:PANEFLOW_BIN_DIR) + $entries) -join $sep
}

function global:__paneflow_cwd_uri {
    $providerPath = (Get-Location).ProviderPath
    if ([string]::IsNullOrEmpty($providerPath)) { return $null }
    try {
        return ([System.Uri]$providerPath).AbsoluteUri
    } catch {
        return $null
    }
}

# PSReadLine owns the pre-exec boundary on PowerShell. Wrap its existing
# entry point after the user's profile has loaded so custom key handlers and
# prompt frameworks stay intact. The accepted line is returned unchanged.
if (-not $global:__paneflow_readline_wrapped -and (Test-Path function:PSConsoleHostReadLine)) {
    $global:__paneflow_prev_readline = $function:PSConsoleHostReadLine
    function global:PSConsoleHostReadLine {
        $__paneflow_line = & $global:__paneflow_prev_readline
        if (-not [string]::IsNullOrWhiteSpace([string]$__paneflow_line)) {
            [Console]::Write("$([char]27)]133;C$([char]7)")
        }
        $__paneflow_line
    }
    $global:__paneflow_readline_wrapped = $true
}

# Capture the CURRENT prompt as a ScriptBlock VALUE (snapshot) via
# `$function:prompt`, NOT `Get-Item function:prompt`. A FunctionInfo from
# Get-Item is a LIVE handle: its `.ScriptBlock` re-resolves to whatever
# `prompt` is at call time, which after we redefine `prompt` below is OUR
# wrapper -- so `& $prev.ScriptBlock` calls the wrapper again, recursing
# forever ("call depth overflow") and the prompt never renders. This bites
# hardest with Starship / oh-my-posh, which also redefine `prompt`. The
# $global:__paneflow_prompt_wrapped guard keeps a re-source from capturing
# our own wrapper as the "previous" prompt.
if (-not $global:__paneflow_prompt_wrapped) {
    $global:__paneflow_prev_prompt = $function:prompt
    function global:prompt {
        $__paneflow_last_exit = $global:LASTEXITCODE
        # Call the wrapped prompt FIRST, while $?/$LASTEXITCODE still reflect
        # the user's last command -- Starship / oh-my-posh read them to render
        # the exit-status segment. Our OSC 7 + PATH bookkeeping runs after.
        $global:LASTEXITCODE = $__paneflow_last_exit
        $__paneflow_out = if ($global:__paneflow_prev_prompt) { & $global:__paneflow_prev_prompt } else { "PS $($executionContext.SessionState.Path.CurrentLocation)> " }
        [Console]::Write("$([char]27)]133;A$([char]7)")
        # OSC 7 with BEL terminator (matches zsh/bash/fish emitters). Use
        # [char]27 instead of `e: Windows PowerShell 5.1 treats `e as a
        # literal "e", which leaks "e]7;..." into the terminal.
        $__paneflow_cwd_uri = __paneflow_cwd_uri
        if ($__paneflow_cwd_uri) {
            [Console]::Write("$([char]27)]7;$__paneflow_cwd_uri$([char]7)")
        }
        __paneflow_path_prepend
        $__paneflow_out
    }
    $global:__paneflow_prompt_wrapped = $true
}
__paneflow_path_prepend
"#;

pub(super) fn resolve_default_shell(configured: Option<&str>) -> String {
    resolve_shell_for_spawn(configured).0
}

pub(super) fn resolve_shell_for_spawn(configured: Option<&str>) -> (String, Option<String>) {
    if let Some(path) = configured {
        if let Some(resolved) = configured_shell_if_usable(path) {
            return (resolved, None);
        }
        log::warn!(
            "Configured default_shell {:?} not found or not executable, \
             falling back to platform defaults",
            path
        );
        let fallback = resolve_default_shell_fallback();
        let notice = format!(
            "default_shell {path:?} is missing or not executable; started {fallback} instead."
        );
        return (fallback, Some(notice));
    }
    (resolve_default_shell_fallback(), None)
}

fn expand_home_prefix(path: &str, home: Option<&std::path::Path>) -> Option<std::path::PathBuf> {
    let rest = path
        .strip_prefix("~/")
        .or_else(|| path.strip_prefix("~\\"))?;
    Some(home?.join(rest))
}

fn is_executable_file(candidate: &std::path::Path) -> bool {
    if !candidate.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        std::ffi::CString::new(candidate.as_os_str().as_bytes())
            .is_ok_and(|path| unsafe { libc::access(path.as_ptr(), libc::X_OK) } == 0)
    }
    #[cfg(windows)]
    {
        true
    }
}

fn configured_shell_if_usable(path: &str) -> Option<String> {
    let expanded = expand_home_prefix(path, dirs::home_dir().as_deref());
    let path = expanded
        .as_deref()
        .and_then(std::path::Path::to_str)
        .unwrap_or(path);
    let has_separator = path.contains('/') || path.contains('\\');
    let candidate: std::path::PathBuf = if has_separator {
        std::path::PathBuf::from(path)
    } else {
        #[cfg(windows)]
        if is_bare_bash_name(path)
            && let Some(git_bash) = find_windows_git_bash_path()
        {
            git_bash
        } else {
            which::which(path)
                .ok()
                .or_else(|| well_known_shell_dir_lookup(path))?
        }
        #[cfg(not(windows))]
        {
            which::which(path)
                .ok()
                .or_else(|| well_known_shell_dir_lookup(path))?
        }
    };
    if is_executable_file(&candidate) {
        Some(candidate.to_string_lossy().into_owned())
    } else {
        None
    }
}

#[cfg(windows)]
fn is_bare_bash_name(name: &str) -> bool {
    !name.contains(['/', '\\'])
        && name
            .to_ascii_lowercase()
            .trim_end_matches(".exe")
            .eq("bash")
}

#[cfg(windows)]
pub(crate) fn find_windows_git_bash() -> Option<String> {
    find_windows_git_bash_path().map(|path| path.to_string_lossy().trim().to_owned())
}

#[cfg(windows)]
fn find_windows_git_bash_path() -> Option<std::path::PathBuf> {
    windows_git_bash_candidates()
        .into_iter()
        .find(|candidate| candidate.is_file())
}

#[cfg(windows)]
fn windows_git_bash_candidates() -> Vec<std::path::PathBuf> {
    let mut candidates = Vec::new();

    for env_var in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(base) = std::env::var_os(env_var) {
            push_git_bash_candidates(&mut candidates, std::path::Path::new(&base).join("Git"));
        }
    }

    if let Ok(git) = which::which("git.exe") {
        candidates.extend(git_bash_candidates_from_git_exe(&git));
    }

    candidates
}

#[cfg(windows)]
fn push_git_bash_candidates(candidates: &mut Vec<std::path::PathBuf>, root: std::path::PathBuf) {
    for candidate in [root.join("bin\\bash.exe"), root.join("usr\\bin\\bash.exe")] {
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
}

#[cfg(windows)]
fn git_bash_candidates_from_git_exe(git: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut candidates = Vec::new();
    let mut dir = git.parent();
    let mut depth = 0;

    while let Some(current) = dir {
        if depth > 4 {
            break;
        }
        push_git_bash_candidates(&mut candidates, current.to_path_buf());
        dir = current.parent();
        depth += 1;
    }

    candidates
}

fn well_known_shell_dir_lookup(name: &str) -> Option<std::path::PathBuf> {
    #[cfg(unix)]
    {
        const DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"];
        DIRS.iter()
            .map(|dir| std::path::Path::new(dir).join(name))
            .find(|candidate| candidate.is_file())
    }
    #[cfg(windows)]
    {
        let lower = name.to_ascii_lowercase();
        match lower.trim_end_matches(".exe") {
            "pwsh" => find_windows_pwsh(),
            "powershell" => windows_powershell_v1_path(),
            "cmd" => windows_cmd_path(),
            _ => None,
        }
    }
}

#[cfg(unix)]
fn resolve_default_shell_fallback() -> String {
    resolve_unix_default_shell_fallback(std::env::var("SHELL").ok().as_deref())
}

#[cfg(unix)]
fn resolve_unix_default_shell_fallback(shell_env: Option<&str>) -> String {
    if let Some(shell) = shell_env
        && let Some(resolved) = configured_shell_if_usable(shell)
    {
        return resolved;
    }
    if let Some(shell) = shell_env
        && !shell.trim().is_empty()
    {
        log::warn!(
            "SHELL {:?} not found or not executable, falling back to /bin/sh",
            shell
        );
    }
    configured_shell_if_usable("/bin/sh").unwrap_or_else(|| "/bin/sh".to_string())
}

#[cfg(windows)]
fn resolve_default_shell_fallback() -> String {
    if let Some(powershell) = find_windows_powershell() {
        return powershell;
    }
    if let Some(cmd) = windows_cmd_path() {
        return cmd.to_string_lossy().into_owned();
    }
    log::error!(
        "Windows shell fallback chain exhausted: no pwsh.exe/powershell.exe found, \
         and %ComSpec% / %SystemRoot%\\System32\\cmd.exe both unavailable. Falling \
         back to bare 'cmd.exe'; PTY spawn will surface a clear error if even this \
         is missing."
    );
    "cmd.exe".to_string()
}

#[cfg(windows)]
fn windows_powershell_v1_path() -> Option<std::path::PathBuf> {
    let exe = windows_system32_dir().join(r"WindowsPowerShell\v1.0\powershell.exe");
    exe.is_file().then_some(exe)
}

#[cfg(windows)]
fn windows_cmd_path() -> Option<std::path::PathBuf> {
    if let Some(com_spec) = std::env::var_os("ComSpec") {
        let path = std::path::PathBuf::from(com_spec);
        if path.is_file() {
            return Some(path);
        }
    }
    let exe = windows_system32_dir().join("cmd.exe");
    exe.is_file().then_some(exe)
}

#[cfg(windows)]
fn windows_system32_dir() -> std::path::PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    std::path::PathBuf::from(root).join("System32")
}

#[cfg(windows)]
fn find_windows_powershell() -> Option<String> {
    find_windows_pwsh()
        .or_else(|| which::which("powershell.exe").ok())
        .or_else(windows_powershell_v1_path)
        .map(|path| path.to_string_lossy().trim().to_owned())
}

#[cfg(windows)]
fn find_windows_pwsh() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;

    static CACHED: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    if let Some(cached) = CACHED.get() {
        return Some(cached.clone());
    }

    fn find_pwsh_in_program_files(env_var: &str) -> Option<PathBuf> {
        let base = PathBuf::from(std::env::var_os(env_var)?).join("PowerShell");
        base.read_dir()
            .ok()?
            .filter_map(Result::ok)
            .filter(|entry| matches!(entry.file_type(), Ok(ft) if ft.is_dir()))
            .filter_map(|entry| {
                let version: u32 = entry.file_name().to_string_lossy().parse().ok()?;
                let exe = entry.path().join("pwsh.exe");
                exe.exists().then_some((version, exe))
            })
            .max_by_key(|(version, _)| *version)
            .map(|(_, exe)| exe)
    }

    fn find_pwsh_in_msix() -> Option<PathBuf> {
        let dir = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Microsoft\\WindowsApps");
        dir.read_dir()
            .ok()?
            .filter_map(Result::ok)
            .filter(|entry| matches!(entry.file_type(), Ok(ft) if ft.is_dir()))
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("Microsoft.PowerShell_")
            })
            .find_map(|entry| {
                let exe = entry.path().join("pwsh.exe");
                exe.exists().then_some(exe)
            })
    }

    fn find_pwsh_in_scoop() -> Option<PathBuf> {
        let exe = PathBuf::from(std::env::var_os("USERPROFILE")?).join("scoop\\shims\\pwsh.exe");
        exe.exists().then_some(exe)
    }

    let found = find_pwsh_in_program_files("ProgramFiles")
        .or_else(|| find_pwsh_in_program_files("ProgramFiles(x86)"))
        .or_else(find_pwsh_in_msix)
        .or_else(find_pwsh_in_scoop)
        .or_else(|| which::which("pwsh.exe").ok())?;
    Some(CACHED.get_or_init(|| found).clone())
}

pub(crate) fn clear_then(command: &str, configured_shell: Option<&str>) -> String {
    clear_then_for_shell(command, &resolve_default_shell(configured_shell))
}

fn clear_then_for_shell(command: &str, shell: &str) -> String {
    let basename = shell
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(shell)
        .to_ascii_lowercase();
    let key = basename.trim_end_matches(".exe");
    match key {
        "cmd" => format!("cls && {command}"),
        "pwsh" | "powershell" => format!("Clear-Host; {command}"),
        "sh" | "bash" | "zsh" | "fish" | "dash" | "ksh" | "ash" | "mksh" => {
            format!("clear && {command}")
        }
        _ => command.to_string(),
    }
}

fn to_shell_path(p: &std::path::Path) -> String {
    let s = p.display().to_string();
    #[cfg(windows)]
    {
        s.replace('\\', "/")
    }
    #[cfg(not(windows))]
    {
        s
    }
}

fn write_integration_script(path: &std::path::Path, contents: &str) -> bool {
    let is_current = |path: &std::path::Path| {
        std::fs::read(path).is_ok_and(|bytes| bytes == contents.as_bytes())
    };
    if is_current(path) {
        return true;
    }
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return false;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return false;
    }
    let staging = parent.join(format!(
        ".{}.{}.{}.tmp",
        name.to_string_lossy(),
        std::process::id(),
        INTEGRATION_WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let written = std::fs::write(&staging, contents).and_then(|()| std::fs::rename(&staging, path));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&staging);
        if is_current(path) {
            return true;
        }
        log::warn!(
            "paneflow: could not write shell integration {}: {error}",
            path.display()
        );
        return false;
    }
    true
}

pub(super) fn setup_shell_integration(
    base: Option<&std::path::Path>,
    shell: &str,
    env: &mut HashMap<String, String>,
) -> Vec<String> {
    let Some(base) = base else {
        return vec![];
    };
    setup_shell_integration_in(base, shell, env)
}

fn setup_shell_integration_in(
    base: &std::path::Path,
    shell: &str,
    env: &mut HashMap<String, String>,
) -> Vec<String> {
    let basename = std::path::Path::new(shell)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(shell);
    let normalized = basename.to_ascii_lowercase();
    let key = normalized.trim_end_matches(".exe");
    match key {
        "zsh" => {
            let dir = base.join("zsh");
            if !write_integration_script(&dir.join(".zshenv"), ZSH_OSC7) {
                return vec![];
            }
            if let Ok(orig) = std::env::var("ZDOTDIR") {
                env.insert("PANEFLOW_ORIG_ZDOTDIR".into(), orig);
            }
            env.insert("ZDOTDIR".into(), dir.display().to_string());
            vec![]
        }
        "bash" => {
            let rcfile = base.join("bash").join("bashrc");
            if !write_integration_script(&rcfile, BASH_OSC7) {
                return vec![];
            }
            vec!["--rcfile".into(), to_shell_path(&rcfile)]
        }
        "fish" => {
            let initfile = base.join("fish").join("osc7.fish");
            if !write_integration_script(&initfile, FISH_OSC7) {
                return vec![];
            }
            vec![
                "--init-command".into(),
                format!("source {}", quote_fish_arg(&to_shell_path(&initfile))),
            ]
        }
        "wsl" => setup_wsl_shell_integration(base),
        "pwsh" | "powershell" => {
            let initfile = base.join("pwsh").join("osc7.ps1");
            if !write_integration_script(&initfile, PWSH_OSC7) {
                return vec![];
            }
            let escaped = initfile.display().to_string().replace('\'', "''");
            powershell_startup_args(format!(". '{escaped}'"))
        }
        "cmd" => {
            log::info!(
                "paneflow: cmd.exe has no OSC 7 scripting hook; split-pane CWD \
                 inheritance from cmd.exe panes is v1-unsupported"
            );
            vec![]
        }
        _ => vec![],
    }
}

fn setup_wsl_shell_integration(base: &std::path::Path) -> Vec<String> {
    let bashrc = base.join("bash").join("bashrc");
    let zshenv = base.join("zsh").join(".zshenv");
    let fish_init = base.join("fish").join("osc7.fish");

    for (path, contents) in [
        (&bashrc, BASH_OSC7),
        (&zshenv, ZSH_OSC7),
        (&fish_init, FISH_OSC7),
    ] {
        if !write_integration_script(path, contents) {
            log::warn!(
                "paneflow: could not materialize WSL shell integration at {}",
                path.display()
            );
            return vec![];
        }
    }

    wsl_startup_args(
        bashrc.display().to_string(),
        zshenv
            .parent()
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
        fish_init.display().to_string(),
    )
}

fn wsl_startup_args(bashrc: String, zdotdir: String, fish_init: String) -> Vec<String> {
    vec![
        "--exec".into(),
        "/bin/sh".into(),
        "-c".into(),
        WSL_SHELL_BOOTSTRAP.into(),
        "paneflow-wsl-bootstrap".into(),
        bashrc,
        zdotdir,
        fish_init,
    ]
}

fn powershell_startup_args(init_command: String) -> Vec<String> {
    vec![
        "-NoLogo".into(),
        "-NoExit".into(),
        "-Command".into(),
        init_command,
    ]
}

fn quote_fish_arg(arg: &str) -> String {
    let mut quoted = String::with_capacity(arg.len() + 2);
    quoted.push('"');
    for ch in arg.chars() {
        match ch {
            '\\' | '"' | '$' => {
                quoted.push('\\');
                quoted.push(ch);
            }
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::{clear_then_for_shell, powershell_startup_args, wsl_startup_args};

    fn replace_atomically(path: &std::path::Path, contents: &str) {
        let staging = path.with_extension("reset");
        std::fs::write(&staging, contents).unwrap();
        std::fs::rename(&staging, path).unwrap();
    }

    #[test]
    fn restoring_ten_panes_never_exposes_an_empty_or_partial_integration_script() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Arc, Barrier};

        let base = tempfile::tempdir().unwrap();
        let rcfile = base.path().join("bash").join("bashrc");
        std::fs::create_dir_all(rcfile.parent().unwrap()).unwrap();
        let stale = "# integration written by an older Paneflow\n".repeat(256);
        std::fs::write(&rcfile, &stale).unwrap();

        let done = Arc::new(AtomicBool::new(false));
        let reads = Arc::new(AtomicUsize::new(0));
        let readers: Vec<_> = (0..2)
            .map(|_| {
                let rcfile = rcfile.clone();
                let stale = stale.clone();
                let done = Arc::clone(&done);
                let reads = Arc::clone(&reads);
                std::thread::spawn(move || {
                    while !done.load(Ordering::Acquire) {
                        if let Ok(text) = std::fs::read_to_string(&rcfile) {
                            assert!(
                                text == stale || text == super::BASH_OSC7,
                                "a shell read a {}-byte partial script",
                                text.len()
                            );
                            reads.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                })
            })
            .collect();

        for _ in 0..40 {
            replace_atomically(&rcfile, &stale);
            let start = Arc::new(Barrier::new(10));
            let panes: Vec<_> = (0..10)
                .map(|_| {
                    let base = base.path().to_path_buf();
                    let start = Arc::clone(&start);
                    std::thread::spawn(move || {
                        start.wait();
                        super::setup_shell_integration_in(
                            &base,
                            "bash",
                            &mut std::collections::HashMap::new(),
                        )
                    })
                })
                .collect();
            for pane in panes {
                assert_eq!(pane.join().unwrap()[0], "--rcfile");
            }
            assert_eq!(std::fs::read_to_string(&rcfile).unwrap(), super::BASH_OSC7);
        }

        done.store(true, Ordering::Release);
        for reader in readers {
            reader.join().unwrap();
        }
        assert!(reads.load(Ordering::Relaxed) > 0);
    }

    #[test]
    fn an_unchanged_integration_script_is_not_rewritten() {
        let base = tempfile::tempdir().unwrap();
        let rcfile = base.path().join("bash").join("bashrc");
        super::setup_shell_integration_in(
            base.path(),
            "bash",
            &mut std::collections::HashMap::new(),
        );
        let written = std::fs::metadata(&rcfile).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));

        super::setup_shell_integration_in(
            base.path(),
            "bash",
            &mut std::collections::HashMap::new(),
        );

        assert_eq!(
            std::fs::metadata(&rcfile).unwrap().modified().unwrap(),
            written
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_bash_hook_percent_encodes_every_byte_of_the_working_directory() {
        use paneflow_terminal_ghostty as ghostty;

        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("a%20b").join("\u{e9}t\u{e9} x");
        std::fs::create_dir_all(&dir).unwrap();
        let rcfile = root.path().join("bashrc");
        std::fs::write(&rcfile, super::BASH_OSC7).unwrap();

        let output = std::process::Command::new("bash")
            .args([
                "--norc",
                "--noprofile",
                "-c",
                "source \"$1\"; cd \"$2\" && __paneflow_osc7",
                "paneflow-osc7-test",
            ])
            .arg(&rcfile)
            .arg(&dir)
            .env("HOME", root.path())
            .env_remove("PANEFLOW_BIN_DIR")
            .output()
            .unwrap();
        let report = String::from_utf8(output.stdout).unwrap();
        assert!(
            report.ends_with("/a%2520b/%C3%A9t%C3%A9%20x\u{7}"),
            "{report:?}"
        );

        let mut terminal = ghostty::DisplayTerminal::new(
            ghostty::WindowSize::new(80, 24, 8, 16).unwrap(),
            100,
            ghostty::TerminalAppearance::default(),
        )
        .unwrap();
        terminal.feed(report.as_bytes()).unwrap();
        let reported: Vec<String> = terminal
            .drain_events()
            .into_iter()
            .filter_map(|event| match event {
                ghostty::BackendEvent::WorkingDirectory(cwd) => Some(cwd),
                _ => None,
            })
            .collect();
        assert_eq!(reported, vec![dir.display().to_string()]);
    }

    #[test]
    fn every_hook_percent_encodes_its_working_directory() {
        assert!(super::ZSH_OSC7.contains("\"$(__paneflow_urlencode \"${PWD}\")\""));
        assert!(super::BASH_OSC7.contains("\"$(__paneflow_urlencode \"${PWD}\")\""));
        assert!(super::FISH_OSC7.contains("(string escape --style=url -- \"$PWD\")"));
    }

    #[test]
    fn default_shell_expands_a_home_relative_path() {
        let home = std::path::Path::new("/home/dev");
        assert_eq!(
            super::expand_home_prefix("~/bin/zsh", Some(home)),
            Some(home.join("bin/zsh"))
        );
        assert_eq!(super::expand_home_prefix("/bin/zsh", Some(home)), None);
        assert_eq!(super::expand_home_prefix("~/bin/zsh", None), None);
    }

    #[test]
    fn a_missing_default_shell_starts_the_platform_shell_and_names_the_refused_path() {
        let refused = "~/definitely-not-a-paneflow-shell";
        let (shell, notice) = super::resolve_shell_for_spawn(Some(refused));

        assert_eq!(shell, super::resolve_default_shell(None));
        let notice = notice.expect("a refused default_shell is reported");
        assert!(notice.contains(refused), "{notice}");
        assert!(notice.contains(&shell), "{notice}");
    }

    #[cfg(unix)]
    #[test]
    fn a_default_shell_without_execute_permission_is_refused() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let shell = dir.path().join("shell");
        std::fs::write(&shell, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!super::is_executable_file(&shell));
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(super::is_executable_file(&shell));
    }

    #[cfg(unix)]
    #[test]
    fn well_known_shell_lookup_finds_sh_and_rejects_bogus() {
        let found = super::well_known_shell_dir_lookup("sh");
        assert!(
            found
                .as_deref()
                .is_some_and(|p| p.is_file() && p.file_name() == Some(std::ffi::OsStr::new("sh"))),
            "a bare `sh` must resolve from the well-known Unix dirs, got {found:?}"
        );
        assert!(
            super::well_known_shell_dir_lookup("definitely-not-a-real-shell-xyz").is_none(),
            "a non-existent bare name must not resolve"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_fallback_rejects_stale_shell_env() {
        let shell = super::resolve_unix_default_shell_fallback(Some(
            "/definitely/not/a/real/paneflow-shell",
        ));
        assert!(
            std::path::Path::new(&shell)
                .file_name()
                .is_some_and(|name| name == std::ffi::OsStr::new("sh")),
            "stale SHELL must fall back to sh, got {shell:?}"
        );
    }

    #[test]
    fn fish_init_command_quotes_spaces_and_metacharacters() {
        assert_eq!(
            super::quote_fish_arg("/Users/a/Application Support/paneflow/osc7.fish"),
            "\"/Users/a/Application Support/paneflow/osc7.fish\""
        );
        assert_eq!(
            super::quote_fish_arg("/tmp/$USER/osc7\"hook\".fish"),
            "\"/tmp/\\$USER/osc7\\\"hook\\\".fish\""
        );
    }

    #[test]
    fn wsl_bootstrap_passes_integration_paths_positionally() {
        let bashrc = r"C:\Users\O'Brien\App Data\$(touch nope)\bashrc";
        let zdotdir = r"C:\Users\O'Brien\App Data\zsh";
        let fish_init = r"C:\Users\O'Brien\App Data\fish\osc7.fish";
        let args = wsl_startup_args(bashrc.into(), zdotdir.into(), fish_init.into());

        assert_eq!(&args[..3], ["--exec", "/bin/sh", "-c"]);
        assert_eq!(args[4], "paneflow-wsl-bootstrap");
        assert_eq!(&args[5..], [bashrc, zdotdir, fish_init]);
        assert!(!args[3].contains(bashrc));
        assert!(!args[3].contains(zdotdir));
        assert!(!args[3].contains(fish_init));
    }

    #[test]
    fn wsl_bootstrap_integrates_known_shells_and_falls_back_safely() {
        let script = super::WSL_SHELL_BOOTSTRAP;
        for shell in ["bash)", "zsh)", "fish)"] {
            assert!(
                script.contains(shell),
                "missing WSL integration for {shell}"
            );
        }
        assert!(script.contains("*)\n        exec \"$shell\""));
        assert!(!script.contains("eval "));
        assert!(script.contains("wslpath -u -- \"$1\""));
        assert!(script.contains("wslpath -u -- \"$2\""));
        assert!(script.contains("wslpath -u -- \"$3\""));
    }

    #[test]
    fn clear_then_uses_cmd_syntax() {
        assert_eq!(
            clear_then_for_shell("codex", r"C:\Windows\System32\cmd.exe"),
            "cls && codex"
        );
        assert_eq!(
            clear_then_for_shell("openclaw tui", r"C:\Windows\System32\cmd.exe"),
            "cls && openclaw tui"
        );
    }

    #[test]
    fn clear_then_uses_powershell_51_compatible_syntax() {
        assert_eq!(
            clear_then_for_shell("claude", "powershell.exe"),
            "Clear-Host; claude"
        );
        assert_eq!(clear_then_for_shell("claude", "pwsh"), "Clear-Host; claude");
        assert_eq!(
            clear_then_for_shell("openclaw tui", "pwsh"),
            "Clear-Host; openclaw tui"
        );
    }

    #[test]
    fn clear_then_uses_posix_syntax_for_unix_shells() {
        assert_eq!(
            clear_then_for_shell("opencode", "/bin/zsh"),
            "clear && opencode"
        );
        assert_eq!(
            clear_then_for_shell("openclaw tui", "/bin/zsh"),
            "clear && openclaw tui"
        );
    }

    #[test]
    fn clear_then_known_posix_shells_keep_clear() {
        for sh in ["/bin/bash", "/usr/bin/fish", "dash", "ksh", "/bin/sh"] {
            assert_eq!(clear_then_for_shell("x", sh), "clear && x", "shell {sh}");
        }
    }

    #[test]
    fn clear_then_unknown_shell_launches_bare() {
        assert_eq!(clear_then_for_shell("opencode", "/usr/bin/nu"), "opencode");
        assert_eq!(clear_then_for_shell("claude", "elvish"), "claude");
    }

    #[test]
    fn pwsh_osc7_snapshots_prompt_and_avoids_recursion() {
        let s = super::PWSH_OSC7;
        assert!(
            s.contains("$global:__paneflow_prev_prompt = $function:prompt"),
            "must snapshot the prompt by value via $function:prompt"
        );
        assert!(
            s.contains("& $global:__paneflow_prev_prompt"),
            "must invoke the captured scriptblock directly (not .ScriptBlock of a live handle)"
        );
        assert!(
            s.contains("__paneflow_prompt_wrapped"),
            "must guard against double-wrapping on re-source"
        );
    }

    #[test]
    fn pwsh_osc7_uses_powershell_51_safe_escape_and_file_uri() {
        let s = super::PWSH_OSC7;
        assert!(
            s.contains("$([char]27)]7;"),
            "OSC 7 must emit ESC via [char]27 for Windows PowerShell 5.1"
        );
        assert!(
            s.contains("$([char]7)"),
            "OSC 7 must emit BEL via [char]7 for Windows PowerShell 5.1"
        );
        assert!(
            s.contains("([System.Uri]$providerPath).AbsoluteUri"),
            "PowerShell CWD reporting must produce a real file:// URI"
        );
        assert!(
            !s.contains("`e]7;"),
            "`e is PowerShell 7-only for ESC and must not be used in shared 5.1/7 script"
        );
    }

    #[test]
    fn shell_integrations_emit_osc133_without_replacing_prompt_hooks() {
        assert!(super::ZSH_OSC7.contains("add-zsh-hook precmd __paneflow_osc133_precmd"));
        assert!(super::ZSH_OSC7.contains("add-zsh-hook preexec __paneflow_osc133_preexec"));
        assert!(super::BASH_OSC7.contains("PROMPT_COMMAND=\"__paneflow_osc133_precmd;"));
        assert!(super::BASH_OSC7.contains("PS0=$'\\e]133;C\\a'"));
        assert!(super::FISH_OSC7.contains("--on-event fish_prompt"));
        assert!(super::PWSH_OSC7.contains("function global:PSConsoleHostReadLine"));
        assert!(super::PWSH_OSC7.contains(")]133;C"));
        assert!(super::PWSH_OSC7.contains(")]133;A"));
        assert!(super::PWSH_OSC7.contains("$global:LASTEXITCODE = $__paneflow_last_exit"));
        for script in [
            super::ZSH_OSC7,
            super::BASH_OSC7,
            super::FISH_OSC7,
            super::PWSH_OSC7,
        ] {
            assert!(!script.contains("133;D"), "no snippet emits OSC 133 D");
        }
    }

    #[test]
    fn powershell_startup_always_loads_the_user_profile() {
        assert_eq!(
            powershell_startup_args("init".into()),
            vec!["-NoLogo", "-NoExit", "-Command", "init"]
        );
    }
}

#[cfg(all(test, windows))]
mod windows_shell_tests {
    use super::*;

    #[test]
    fn fallback_returns_nonempty_shell() {
        assert!(
            !resolve_default_shell_fallback().is_empty(),
            "Windows shell fallback must never return an empty string"
        );
    }

    #[test]
    fn fallback_prefers_powershell_over_cmd_when_present() {
        if find_windows_powershell().is_some() {
            let shell = resolve_default_shell_fallback().to_ascii_lowercase();
            assert!(
                shell.ends_with("pwsh.exe") || shell.ends_with("powershell.exe"),
                "expected the default to be a PowerShell, got {shell:?}"
            );
        }
    }

    #[test]
    fn discovered_powershell_is_pwsh_or_powershell() {
        if let Some(found) = find_windows_powershell() {
            let stem = std::path::Path::new(&found)
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_ascii_lowercase);
            assert!(
                matches!(stem.as_deref(), Some("pwsh") | Some("powershell")),
                "unexpected PowerShell binary stem: {found:?}"
            );
        }
    }

    #[test]
    fn bare_bash_names_are_detected_without_catching_explicit_paths() {
        assert!(is_bare_bash_name("bash"));
        assert!(is_bare_bash_name("bash.exe"));
        assert!(!is_bare_bash_name(r"C:\Windows\System32\bash.exe"));
        assert!(!is_bare_bash_name("zsh"));
    }

    #[test]
    fn git_bash_candidates_are_derived_from_git_cmd_shim() {
        let candidates = git_bash_candidates_from_git_exe(std::path::Path::new(
            r"C:\Program Files\Git\cmd\git.exe",
        ));

        assert!(
            candidates.contains(&std::path::PathBuf::from(
                r"C:\Program Files\Git\bin\bash.exe"
            )),
            "Git for Windows cmd shim should lead to the interactive Git Bash binary"
        );
        assert!(
            candidates.contains(&std::path::PathBuf::from(
                r"C:\Program Files\Git\usr\bin\bash.exe"
            )),
            "Git for Windows cmd shim should also probe the usr/bin bash fallback"
        );
    }

    #[test]
    fn configured_bare_bash_prefers_git_bash_when_installed() {
        let Some(git_bash) = find_windows_git_bash() else {
            eprintln!("skip: Git for Windows bash.exe not found");
            return;
        };

        assert_eq!(
            configured_shell_if_usable("bash.exe").map(|s| s.to_ascii_lowercase()),
            Some(git_bash.to_ascii_lowercase()),
            "bare bash.exe must resolve to Git Bash before Windows' WSL bash launcher"
        );
    }

    #[test]
    fn windows_powershell_51_resolves_without_path() {
        let found = windows_powershell_v1_path();
        assert!(
            found
                .as_deref()
                .is_some_and(|p| p.is_file() && p.ends_with("powershell.exe")),
            "Windows PowerShell 5.1 must resolve from its absolute System32 home, got {found:?}"
        );
    }

    #[test]
    fn well_known_lookup_resolves_the_exact_windows_shell_requested() {
        if let Some(pwsh) = well_known_shell_dir_lookup("pwsh.exe") {
            let lower = pwsh.to_string_lossy().to_ascii_lowercase();
            assert!(
                lower.ends_with(r"\pwsh.exe"),
                "a configured `pwsh.exe` must resolve to pwsh, got {lower}"
            );
        }

        let powershell = well_known_shell_dir_lookup("powershell");
        let lower = powershell
            .as_ref()
            .map(|p| p.to_string_lossy().to_ascii_lowercase());
        assert!(
            lower
                .as_deref()
                .is_some_and(|p| p.ends_with(r"windowspowershell\v1.0\powershell.exe")),
            "a configured `powershell` must resolve to Windows PowerShell 5.1, got {lower:?}"
        );

        let cmd = well_known_shell_dir_lookup("cmd.exe");
        assert!(
            cmd.as_deref()
                .is_some_and(|p| p.is_file() && p.ends_with("cmd.exe")),
            "a configured `cmd.exe` must resolve, got {cmd:?}"
        );

        assert!(
            well_known_shell_dir_lookup("definitely-not-a-real-shell-xyz").is_none(),
            "an unknown bare name must not resolve to some other shell"
        );
    }

    #[test]
    fn pwsh_discovery_is_stable_across_calls() {
        assert_eq!(find_windows_pwsh(), find_windows_pwsh());
    }
}
