use std::collections::HashMap;

const ZSH_OSC7: &str = r#"# PaneFlow shell integration - OSC 7 CWD reporting
if [[ -n "${PANEFLOW_ORIG_ZDOTDIR+x}" ]]; then
    ZDOTDIR="${PANEFLOW_ORIG_ZDOTDIR}"
    unset PANEFLOW_ORIG_ZDOTDIR
else
    unset ZDOTDIR
fi
[[ -f "${ZDOTDIR:-$HOME}/.zshenv" ]] && source "${ZDOTDIR:-$HOME}/.zshenv"
__paneflow_osc7() { printf '\e]7;file://%s%s\a' "${HOST}" "${PWD}"; }
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
        local ret=$?
        if [[ -n "${__paneflow_cmd_ran-}" ]]; then
            printf '\e]133;D;%s\a' "${ret}"
            unset __paneflow_cmd_ran
        fi
        printf '\e]133;A\a'
    }
    __paneflow_osc133_preexec() {
        __paneflow_cmd_ran=1
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
__paneflow_osc7() { printf '\e]7;file://%s%s\a' "${HOSTNAME}" "${PWD}"; }
__paneflow_path_prepend() {
    [[ -z "${PANEFLOW_BIN_DIR-}" ]] && return
    local p=":${PATH}:"
    p="${p//:${PANEFLOW_BIN_DIR}:/:}"
    p="${p#:}"; p="${p%:}"
    PATH="${PANEFLOW_BIN_DIR}:${p}"
    export PATH
}
__paneflow_osc133_precmd() {
    local ret=$?
    if [[ "${HISTCMD-0}" != "${__paneflow_histcmd-}" ]]; then
        [[ -n "${__paneflow_histcmd-}" ]] && printf '\e]133;D;%s\a' "${ret}"
        __paneflow_histcmd="${HISTCMD-0}"
    fi
    printf '\e]133;A\a'
}
PS0=$'\e]133;C\a'"${PS0-}"
PROMPT_COMMAND="__paneflow_osc133_precmd;__paneflow_osc7;__paneflow_path_prepend${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
__paneflow_path_prepend
"#;

const FISH_OSC7: &str = r#"# PaneFlow shell integration - OSC 7 CWD reporting
function __paneflow_osc7 --on-variable PWD
    printf '\e]7;file://%s%s\a' (hostname) "$PWD"
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
    function __paneflow_osc133_postexec --on-event fish_postexec
        printf '\e]133;D;%s\a' $status
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
        $__paneflow_ok = $?
        $__paneflow_last_exit = $global:LASTEXITCODE
        $__paneflow_history = (Get-History -Count 1).Id
        # Call the wrapped prompt FIRST, while $?/$LASTEXITCODE still reflect
        # the user's last command -- Starship / oh-my-posh read them to render
        # the exit-status segment. Our OSC 7 + PATH bookkeeping runs after.
        $global:LASTEXITCODE = $__paneflow_last_exit
        $__paneflow_out = if ($global:__paneflow_prev_prompt) { & $global:__paneflow_prev_prompt } else { "PS $($executionContext.SessionState.Path.CurrentLocation)> " }
        if ($null -ne $global:__paneflow_previous_history -and $__paneflow_history -ne $global:__paneflow_previous_history) {
            $__paneflow_code = if ($__paneflow_ok) { 0 } elseif ($null -ne $__paneflow_last_exit) { $__paneflow_last_exit } else { 1 }
            [Console]::Write("$([char]27)]133;D;$__paneflow_code$([char]7)")
        }
        $global:__paneflow_previous_history = $__paneflow_history
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
    if let Some(path) = configured {
        if let Some(resolved) = configured_shell_if_usable(path) {
            return resolved;
        }
        log::warn!(
            "Configured default_shell {:?} not found or not executable, \
             falling back to platform defaults",
            path
        );
    }
    resolve_default_shell_fallback()
}

fn configured_shell_if_usable(path: &str) -> Option<String> {
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
    let is_executable = candidate.is_file() && {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(&candidate)
                .map(|m| m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        }
        #[cfg(windows)]
        {
            std::fs::metadata(&candidate).is_ok()
        }
    };
    if is_executable {
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

pub(super) fn setup_shell_integration(
    shell: &str,
    env: &mut HashMap<String, String>,
) -> Vec<String> {
    let Some(base) = crate::runtime_paths::shell_integration_dir() else {
        return vec![];
    };

    let basename = std::path::Path::new(shell)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(shell);
    let normalized = basename.to_ascii_lowercase();
    let key = normalized.trim_end_matches(".exe");
    match key {
        "zsh" => {
            let dir = base.join("zsh");
            if std::fs::create_dir_all(&dir).is_err() {
                return vec![];
            }
            if std::fs::write(dir.join(".zshenv"), ZSH_OSC7).is_err() {
                return vec![];
            }
            if let Ok(orig) = std::env::var("ZDOTDIR") {
                env.insert("PANEFLOW_ORIG_ZDOTDIR".into(), orig);
            }
            env.insert("ZDOTDIR".into(), dir.display().to_string());
            vec![]
        }
        "bash" => {
            let dir = base.join("bash");
            if std::fs::create_dir_all(&dir).is_err() {
                return vec![];
            }
            let rcfile = dir.join("bashrc");
            if std::fs::write(&rcfile, BASH_OSC7).is_err() {
                return vec![];
            }
            vec!["--rcfile".into(), to_shell_path(&rcfile)]
        }
        "fish" => {
            let dir = base.join("fish");
            if std::fs::create_dir_all(&dir).is_err() {
                return vec![];
            }
            let initfile = dir.join("osc7.fish");
            if std::fs::write(&initfile, FISH_OSC7).is_err() {
                return vec![];
            }
            vec![
                "--init-command".into(),
                format!("source {}", quote_fish_arg(&to_shell_path(&initfile))),
            ]
        }
        "wsl" => setup_wsl_shell_integration(&base),
        "pwsh" | "powershell" => {
            let dir = base.join("pwsh");
            if std::fs::create_dir_all(&dir).is_err() {
                return vec![];
            }
            let initfile = dir.join("osc7.ps1");
            if std::fs::write(&initfile, PWSH_OSC7).is_err() {
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
        let Some(parent) = path.parent() else {
            return vec![];
        };
        if std::fs::create_dir_all(parent).is_err() || std::fs::write(path, contents).is_err() {
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
    vec!["-NoExit".into(), "-Command".into(), init_command]
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
            clear_then_for_shell("kiro-cli chat", "pwsh"),
            "Clear-Host; kiro-cli chat"
        );
    }

    #[test]
    fn clear_then_uses_posix_syntax_for_unix_shells() {
        assert_eq!(
            clear_then_for_shell("opencode", "/bin/zsh"),
            "clear && opencode"
        );
        assert_eq!(
            clear_then_for_shell("kiro-cli chat", "/bin/zsh"),
            "clear && kiro-cli chat"
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
        assert!(super::FISH_OSC7.contains("--on-event fish_postexec"));
        assert!(super::PWSH_OSC7.contains("function global:PSConsoleHostReadLine"));
        assert!(super::PWSH_OSC7.contains(")]133;C"));
        assert!(super::PWSH_OSC7.contains(")]133;D;"));
        assert!(super::PWSH_OSC7.contains(")]133;A"));
    }

    #[test]
    fn powershell_startup_always_loads_the_user_profile() {
        assert_eq!(
            powershell_startup_args("init".into()),
            vec!["-NoExit", "-Command", "init"]
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
