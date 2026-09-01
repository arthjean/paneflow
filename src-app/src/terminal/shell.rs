//! Shell resolution + automatic OSC 7 injection.
//!
//! `resolve_default_shell` picks the shell binary to launch in every PTY,
//! following a platform-specific fallback chain. `setup_shell_integration`
//! writes small per-shell rc scripts into Paneflow's local data dir
//! (`runtime_paths::shell_integration_dir`) and returns the extra CLI
//! args/env needed to wire them in.
//!
//! Keep this module shell-specific: no terminal state, no GPUI.

use std::collections::HashMap;

/// zsh: ZDOTDIR-based injection. Our `.zshenv` restores the original ZDOTDIR
/// so all other dotfiles (`.zshrc`, `.zprofile`) load from `$HOME` as usual.
///
/// AI-hook PATH-prepend (re-applied via `precmd`): the PTY-level
/// `$PATH` prepend in `pty_session::inject_ai_hook_env` is invariably
/// undone by user `.zshrc`/`.bashrc` lines like
/// `export PATH="$HOME/.local/bin:$PATH"`, which demote PaneFlow's bin
/// dir behind the user's `~/.local/bin/claude` and bypass the shim
/// entirely. We re-prepend before every prompt - first invocation runs
/// after `.zshrc` finishes, so the first `claude` typed at the prompt
/// resolves to the shim. Idempotent + O(1) string work, invisible cost.
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

/// bash: `--rcfile` replacement. Sources the real `.bashrc`, then appends
/// our OSC 7 function to PROMPT_COMMAND (preserving starship/oh-my-bash/etc.).
/// Same AI-hook PATH-prepend rationale as ZSH_OSC7 - PROMPT_COMMAND fires
/// before each prompt, after `.bashrc` has run.
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

/// fish: `--init-command` sourced script. Uses `--on-variable PWD` so it
/// fires on every directory change independently of the prompt function.
/// fish `--init-command` runs AFTER `config.fish`, so a one-shot prepend
/// is sufficient - but `fish_add_path -gp` is idempotent so a re-source
/// of this file is also safe.
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

/// WSL bootstrap for the configured default distribution. `wsl.exe` only
/// identifies the Windows launcher, so the Linux login shell is resolved
/// inside the distribution. Integration paths stay positional arguments:
/// no user-controlled path is interpolated into this script.
///
/// The integrated shell is deliberately interactive and non-login, matching
/// PaneFlow's native Bash/Zsh/Fish launch contract. This preserves `.bashrc`,
/// `.zshrc`, and `config.fish`; login-only profile files are not evaluated.
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

/// PowerShell 5.1 / 7 (pwsh): dot-sourced via `-NoExit -Command ". <path>"`,
/// which runs AFTER the user's `$PROFILE`, so any `prompt` function they
/// defined is already in place. We capture it as a ScriptBlock and wrap it
/// non-destructively so their prompt still renders while we emit OSC 7.
///
/// BEL terminator (``a``) matches the zsh/bash/fish emitters so PaneFlow's
/// shared OSC 7 parser handles Windows and Unix identically.
///
/// US-012 - prd-windows-port.md.
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

/// Resolve the default shell path following a platform-specific fallback chain
/// (US-006 - prd-windows-port.md). Returns the path that should be passed to
/// `portable-pty`'s `CommandBuilder::new`.
///
/// Unix chain: configured (if executable) → `$SHELL` → `/bin/sh`.
/// Windows chain: configured (if present; a bare name is resolved via PATH,
/// then via the well-known absolute location for that exact shell - see
/// [`well_known_shell_dir_lookup`]) → PowerShell 7 (`pwsh.exe`) → Windows
/// PowerShell 5.1 (`powershell.exe`, PATH then its System32 home) →
/// `%ComSpec%` → `%SystemRoot%\System32\cmd.exe` → bare `"cmd.exe"`
/// (last-ditch). PowerShell is preferred over `cmd.exe` so a fresh Windows
/// install lands on a modern shell (rich prompt, ANSI colors, working `clear`)
/// instead of the legacy console - mirrors Zed's `get_windows_system_shell`
/// (`crates/util/src/shell.rs`).
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

/// Validate that a user-configured shell entry resolves to an executable file.
/// Bare names (no path separators) are searched on PATH via `which` - this is
/// what lets `"default_shell": "pwsh.exe"` work on Windows without the user
/// having to hard-code `C:\Program Files\PowerShell\7\pwsh.exe`.
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
            // PATH search first; on Unix, fall back to well-known install dirs so a
            // bare `"pwsh"` configured shell still resolves under a GUI launch whose
            // inherited PATH omits `/opt/homebrew/bin` (the macOS parallel to the
            // Windows `find_windows_powershell` well-known-location probe). Without
            // this, the entry was silently rejected and the shell fell back to
            // `/bin/sh`.
            which::which(path)
                .ok()
                .or_else(|| well_known_shell_dir_lookup(path))?
        }
        #[cfg(not(windows))]
        {
            // PATH search first; on Unix, fall back to well-known install dirs so a
            // bare `"pwsh"` configured shell still resolves under a GUI launch whose
            // inherited PATH omits `/opt/homebrew/bin` (the macOS parallel to the
            // Windows `find_windows_powershell` well-known-location probe). Without
            // this, the entry was silently rejected and the shell fell back to
            // `/bin/sh`.
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

/// Probe a small set of well-known install directories for a bare shell name
/// that the PATH search (`which`) missed, so a configured `"pwsh"` / `"fish"` /
/// etc. resolves even when a GUI-launched process inherited a minimal PATH.
/// The executable-bit check is left to the caller.
///
/// Unix covers the Homebrew prefixes (`/opt/homebrew/bin` on Apple Silicon,
/// `/usr/local/bin` on Intel) plus the system dirs.
///
/// Windows maps the bare name onto the same absolute locations the
/// *unconfigured* fallback probes, honoring the exact shell the user picked:
/// `pwsh` never silently becomes Windows PowerShell 5.1, and `powershell` never
/// silently becomes pwsh 7. This arm used to return `None`, on the claim that
/// `which` + `find_windows_powershell` already served the configured-bare-name
/// case. They do not: `find_windows_powershell` is only reachable from
/// `resolve_default_shell_fallback`, i.e. *after* the configured entry has been
/// rejected. So an explicit Settings choice (the dropdown persists bare
/// `"pwsh.exe"` / `"powershell.exe"`) was strictly less robust than "System
/// default" whenever `PATH` was stale - an Explorer-launched process that
/// predates the shell's install, or a `PATH` long enough to be truncated - and
/// the user's pick was silently swapped for another shell.
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
        // Bound before the `match`: edition 2024 drops a scrutinee temporary
        // before the arms run, so the lowercased name needs to outlive it.
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
    // Prefer PowerShell over cmd.exe. A bare cmd.exe default gives the legacy
    // "BIOS console" experience - no `clear` (it's `cls`), a 16-color `C:\>`
    // prompt, no PSReadLine - which is jarring next to a standalone PowerShell.
    // Mirrors Zed's `get_windows_system_shell` (crates/util/src/shell.rs):
    // pwsh 7 → Windows PowerShell 5.1 → cmd.exe only as a last resort.
    if let Some(powershell) = find_windows_powershell() {
        return powershell;
    }
    // No PowerShell found - fall back to cmd.exe (%ComSpec%, then the canonical
    // System32 path).
    if let Some(cmd) = windows_cmd_path() {
        return cmd.to_string_lossy().into_owned();
    }
    // Last-ditch: return bare "cmd.exe" and let the spawner search PATH.
    log::error!(
        "Windows shell fallback chain exhausted: no pwsh.exe/powershell.exe found, \
         and %ComSpec% / %SystemRoot%\\System32\\cmd.exe both unavailable. Falling \
         back to bare 'cmd.exe'; PTY spawn will surface a clear error if even this \
         is missing."
    );
    "cmd.exe".to_string()
}

/// The absolute home of Windows PowerShell 5.1. Probed because
/// `which("powershell.exe")` only finds it when
/// `%SystemRoot%\System32\WindowsPowerShell\v1.0` is on `PATH`, which is its own
/// entry and can drop out of a stale or truncated environment. Zed's
/// `get_windows_system_shell` ends its chain on the same absolute path before
/// giving up on cmd.exe (`crates/gpui_util/src/lib.rs`); without it, a machine
/// with a healthy PowerShell install can still land on the legacy console.
#[cfg(windows)]
fn windows_powershell_v1_path() -> Option<std::path::PathBuf> {
    let exe = windows_system32_dir().join(r"WindowsPowerShell\v1.0\powershell.exe");
    exe.is_file().then_some(exe)
}

/// `%ComSpec%` - the Windows convention for "the command interpreter",
/// respected by every console app - then the canonical 64-bit `cmd.exe`. WOW64
/// callers still see cmd.exe under System32 via redirection.
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

/// `%SystemRoot%\System32`, defaulting to the canonical location when the env
/// var is missing (a GUI launch can inherit a surprisingly bare environment).
#[cfg(windows)]
fn windows_system32_dir() -> std::path::PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    std::path::PathBuf::from(root).join("System32")
}

/// Locate a PowerShell executable, preferring PowerShell 7+ (`pwsh.exe`) over
/// the bundled Windows PowerShell 5.1 (`powershell.exe`). Mirrors the search
/// order of Zed's `get_windows_system_shell` so PaneFlow lands on the same
/// modern shell users expect (rich prompt, ANSI colors, working `clear`)
/// rather than cmd.exe.
///
/// Order (short-circuits on the first hit): every [`find_windows_pwsh`]
/// location, then `powershell.exe` on `PATH`, then Windows PowerShell 5.1 at
/// its absolute System32 home.
#[cfg(windows)]
fn find_windows_powershell() -> Option<String> {
    find_windows_pwsh()
        .or_else(|| which::which("powershell.exe").ok())
        .or_else(windows_powershell_v1_path)
        .map(|path| path.to_string_lossy().trim().to_owned())
}

/// Locate PowerShell 7+ (`pwsh.exe`) only, never degrading to Windows
/// PowerShell 5.1 - callers that want that degradation compose it themselves.
/// `pwsh.exe` is frequently NOT on `PATH`, so the well-known install locations
/// are probed before the `PATH` search.
///
/// Order (short-circuits on the first hit):
/// 1. `pwsh.exe` under `%ProgramFiles%\PowerShell\<n>` (highest major version)
/// 2. `pwsh.exe` under `%ProgramFiles(x86)%\PowerShell\<n>`
/// 3. `pwsh.exe` from the MSIX/Store install (`%LOCALAPPDATA%\…\WindowsApps`)
/// 4. `pwsh.exe` from a scoop shim
/// 5. `pwsh.exe` anywhere on `PATH`
///
/// A successful probe is memoized process-wide, as Zed does with its
/// `LazyLock` (`crates/gpui_util/src/lib.rs`). Every spawn resolves the shell
/// on the render thread (`TerminalState::resolve_spawn_params_with_profile`),
/// so without this an N-pane restore ran N sets of `read_dir` + `PATH` walks
/// while the boot-time git scanners and file watchers were saturating the
/// disk, which is exactly the contention window where a transient failure
/// makes the whole chain fall through to cmd.exe. A *failed* probe is
/// deliberately not cached: the answer must be allowed to change once the
/// machine settles.
#[cfg(windows)]
fn find_windows_pwsh() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;

    static CACHED: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    if let Some(cached) = CACHED.get() {
        return Some(cached.clone());
    }

    // Newest `pwsh.exe` under a `<ProgramFiles>\PowerShell` install. The
    // directory names are the major version (`7`, `6`, …); the highest wins.
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

    // Store/MSIX install drops `pwsh.exe` under a versioned package dir.
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

    // scoop shim.
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

/// Build a command that clears the terminal before launching an interactive
/// program, using syntax supported by the shell that will own the PTY.
///
/// In particular, Windows PowerShell 5.1 does not support `&&`, and `cmd.exe`
/// spells the clear command `cls`. When no shell is configured, the platform
/// fallback is resolved before selecting syntax.
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
        // Known POSIX shells: `clear` + `&&` sequencing is universally
        // supported (fish ≥3.0 included).
        "sh" | "bash" | "zsh" | "fish" | "dash" | "ksh" | "ash" | "mksh" => {
            format!("clear && {command}")
        }
        // US-042: unknown shell (nushell, elvish, xonsh, …) - don't assume
        // `&&`/`clear` exist. Launch the command bare so an exotic shell
        // doesn't eat a syntax error on the very first line.
        _ => command.to_string(),
    }
}

/// Render a filesystem path for a POSIX shell's rcfile/init argument.
///
/// US-042: on Windows, bash and fish run under an MSYS / Git-Bash / WSL
/// environment that expects forward-slash paths, even though the host
/// filesystem reports `\`. Converting here keeps `--rcfile` / `source` from
/// receiving an unparseable backslash path. No-op on Unix.
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

/// Write OSC 7 shell integration scripts and return the extra shell args
/// and env vars needed to activate them. Scripts are written to
/// `runtime_paths::shell_integration_dir()/{zsh,bash,fish,pwsh}/`.
///
/// Supported shells:
/// - **zsh, bash, fish** - BEL-terminated OSC 7 via per-prompt hooks.
/// - **WSL** - resolves the distribution's Bash/Zsh/Fish login shell and
///   activates the matching integration without modifying Linux dotfiles.
/// - **PowerShell 5.1 / pwsh 7** (US-012) - `prompt` function wrapper,
///   dot-sourced so the user's `$PROFILE`-defined prompt still renders.
/// - **cmd.exe** - `info!` log only; cmd has no per-prompt scripting hook,
///   so split-pane CWD inheritance from a cmd.exe pane is v1-unsupported
///   (documented in `docs/WINDOWS.md` per US-022).
/// - **Shells without injection** (nushell, elvish, xonsh): rely on
///   `cwd_now()` fallback. On macOS this requires `proc_pidinfo()`.
pub(super) fn setup_shell_integration(
    shell: &str,
    env: &mut HashMap<String, String>,
) -> Vec<String> {
    let Some(base) = crate::runtime_paths::shell_integration_dir() else {
        return vec![];
    };

    // US-006 - `Path::file_name()` is path-separator-agnostic:
    //   /bin/zsh  → "zsh"      (Unix)
    //   C:\Windows\System32\cmd.exe → "cmd.exe"  (Windows)
    //   zsh (bare) → "zsh"     (either platform)
    let basename = std::path::Path::new(shell)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(shell);
    // US-012 - normalize for case-insensitive match + optional `.exe`
    // suffix. Windows allows `pwsh` and `pwsh.exe` interchangeably on
    // PATH; Unix shell names (lowercase, no suffix) are unaffected.
    let normalized = basename.to_ascii_lowercase();
    let key = normalized.trim_end_matches(".exe");
    match key {
        "zsh" => {
            let dir = base.join("zsh");
            if std::fs::create_dir_all(&dir).is_err() {
                return vec![];
            }
            // U-022: if the rc write fails, abort activation rather than
            // hijacking ZDOTDIR to point at a dir with no `.zshenv` - that
            // would suppress the user's real zsh startup AND give no
            // integration. Bail before touching `env`.
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
            // U-022: abort if the write fails - handing bash `--rcfile <path>`
            // for a file that doesn't exist breaks startup instead of
            // gracefully falling back to the user's normal `.bashrc`.
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
            // U-022: abort if the write fails - sourcing a missing init file
            // errors fish startup rather than degrading cleanly.
            if std::fs::write(&initfile, FISH_OSC7).is_err() {
                return vec![];
            }
            vec![
                "--init-command".into(),
                format!("source {}", quote_fish_arg(&to_shell_path(&initfile))),
            ]
        }
        "wsl" => setup_wsl_shell_integration(&base),
        // US-012 - PowerShell 7 (pwsh) and Windows PowerShell 5.1 share
        // the same `function prompt { ... }` hook mechanism, so one
        // script serves both. `-NoExit` keeps the shell interactive after
        // the init command; `-Command ". 'path'"` dot-sources our script
        // AFTER the user's `$PROFILE` has loaded any `prompt` they
        // defined (so we can wrap rather than replace it).
        "pwsh" | "powershell" => {
            let dir = base.join("pwsh");
            if std::fs::create_dir_all(&dir).is_err() {
                return vec![];
            }
            let initfile = dir.join("osc7.ps1");
            // U-022: abort if the write fails - dot-sourcing a missing script
            // breaks the pwsh session rather than degrading cleanly.
            if std::fs::write(&initfile, PWSH_OSC7).is_err() {
                return vec![];
            }
            // Single-quote the path and escape any embedded single
            // quotes ('' is the literal single-quote inside a single-
            // quoted PowerShell string). Guards against pathological
            // usernames without breaking the common case.
            let escaped = initfile.display().to_string().replace('\'', "''");
            powershell_startup_args(format!(". '{escaped}'"))
        }
        // US-012 AC-5 - cmd.exe has no scripting hook for per-prompt
        // actions (its `$PROMPT` env var controls only the displayed
        // text, not arbitrary execution). Split-pane CWD inheritance
        // from cmd.exe panes is v1-unsupported; users can `cd` manually
        // or switch to PowerShell for the integrated experience.
        "cmd" => {
            log::info!(
                "paneflow: cmd.exe has no OSC 7 scripting hook; split-pane CWD \
                 inheritance from cmd.exe panes is v1-unsupported (docs/WINDOWS.md)"
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

/// Start pwsh so it loads the user's `$PROFILE`, on every surface including an
/// agent one.
///
/// Agent surfaces used to add `-NoProfile`, to keep a chatty profile from
/// writing over the frame an agent TUI is about to draw. That traded a
/// permanent defect for a transient one. An agent pane is only an agent pane
/// while the agent runs: quit it and what remains is an interactive shell, and
/// with `-NoProfile` that shell has none of the user's prompt, aliases,
/// functions, or PSReadLine configuration. Ours was the only platform doing
/// this - the zsh, bash, fish and wsl arms never consulted the surface profile
/// at all, so the same agent pane kept its `.zshrc` on Linux and macOS. And it
/// contradicted this module's own OSC 7 design, which dot-sources the
/// integration script *after* `$PROFILE` precisely so a user-defined `prompt`
/// can be wrapped rather than replaced; `-NoProfile` left nothing to wrap.
///
/// The first-frame concern is already handled: `clear_then` prefixes the agent
/// command with `Clear-Host`, so a profile's banner is wiped whether or not it
/// ran.
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

    // (B) Unix well-known-dir shell lookup: a bare name not on PATH still
    // resolves from a standard install dir (the macOS pwsh-under-Homebrew gap),
    // while a bogus name yields None. `/bin/sh` exists on every Unix target.
    #[cfg(unix)]
    #[test]
    fn well_known_shell_lookup_finds_sh_and_rejects_bogus() {
        // Resolves to a real `sh` file from some standard dir - the exact dir
        // varies (`/bin/sh` on macOS, `/usr/bin/sh` on many Linux distros), so
        // assert the basename, not the full path.
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
        // US-042: an unknown shell gets no clear prefix - we can't assume `&&`
        // or `clear` exist (nushell, elvish, xonsh, …).
        assert_eq!(clear_then_for_shell("opencode", "/usr/bin/nu"), "opencode");
        assert_eq!(clear_then_for_shell("claude", "elvish"), "claude");
    }

    #[test]
    fn pwsh_osc7_snapshots_prompt_and_avoids_recursion() {
        // Regression guard for the infinite-recursion bug that left the prompt
        // blank under Starship / oh-my-posh: capturing the previous prompt via a
        // live `Get-Item function:prompt` handle made `.ScriptBlock` re-resolve
        // to our own wrapper after redefinition -> "call depth overflow". The
        // fix snapshots the scriptblock by value (`$function:prompt`), invokes
        // it directly, and guards against re-wrapping.
        //
        // Asserted POSITIVELY (presence of the fixed code lines) rather than by
        // substring-absence: the anti-pattern strings (`Get-Item`,
        // `.ScriptBlock`) legitimately appear in this constant's own
        // explanatory comment, so an absence check would false-positive.
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

    /// Agent surfaces once passed `-NoProfile`, which left the shell the user
    /// gets back after quitting the agent stripped of their prompt, aliases and
    /// PSReadLine setup. Nothing may reintroduce it: pwsh must start the same
    /// way for every surface, and it must load `$PROFILE` so the OSC 7 script
    /// dot-sourced afterwards can wrap a user-defined `prompt` instead of
    /// replacing it.
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

    /// The fallback must always yield a non-empty program for `portable-pty`,
    /// even on a machine with no PowerShell at all (it lands on cmd.exe).
    #[test]
    fn fallback_returns_nonempty_shell() {
        assert!(
            !resolve_default_shell_fallback().is_empty(),
            "Windows shell fallback must never return an empty string"
        );
    }

    /// The regression guard for the "BIOS terminal" bug: whenever a PowerShell
    /// is discoverable (GitHub's `windows-latest` runners ship pwsh 7; any real
    /// Windows box has at least Windows PowerShell 5.1), the default must NOT
    /// degrade to cmd.exe.
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

    /// Whatever `find_windows_powershell` returns must actually be a PowerShell
    /// binary (`pwsh` or `powershell`), never something else mis-classified.
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

    /// Windows PowerShell 5.1 ships with the OS, so its absolute System32 home
    /// must always resolve. This is the probe that keeps a machine with a
    /// truncated or stale `PATH` off cmd.exe: `which("powershell.exe")` needs
    /// `%SystemRoot%\System32\WindowsPowerShell\v1.0` to be a `PATH` entry of
    /// its own, and that entry can be missing while the binary is right there.
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

    /// The configured-bare-name regression guard. The Settings dropdown
    /// persists bare `"pwsh.exe"` / `"powershell.exe"` / `"cmd.exe"`, so the
    /// well-known lookup must answer for each of them, and must answer with the
    /// *exact* shell asked for: `pwsh` never degrades to Windows PowerShell
    /// 5.1 here, and `powershell` never silently becomes pwsh 7. Degrading is
    /// the unconfigured fallback's job, not the configured path's.
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

    /// The pwsh probe memoizes its success, so repeated spawns must keep
    /// answering the same path rather than re-walking `ProgramFiles` + `PATH`
    /// once per pane.
    #[test]
    fn pwsh_discovery_is_stable_across_calls() {
        assert_eq!(find_windows_pwsh(), find_windows_pwsh());
    }
}
