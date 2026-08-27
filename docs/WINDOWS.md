# PaneFlow on Windows

User guide for the v1 Windows release. Covers supported versions,
install paths, known v1 limitations, tracked upstream risks, and how
to report a Windows-specific bug. If you're on Linux or macOS, see
the top-level [README](../README.md) instead.

Maintained alongside the release smoke-test runbook in
[`WINDOWS-SMOKE-TEST.md`](WINDOWS-SMOKE-TEST.md). The PaneFlow website links
directly to this file on GitHub rather than re-hosting the content - one source
of truth, no stale mirror.

---

## 1. Supported Windows versions

| OS | Build floor | Architecture | Status |
|----|-------------|--------------|--------|
| Windows 10 | **1809** (October 2018 Update, build 17763) | x86_64 | Supported |
| Windows 11 | 22H2+ recommended | x86_64 | Supported |
| Windows 10 / 11 | - | ARM64 (`aarch64-pc-windows-msvc`) | **Not in v1** - deferred pending GPUI DX11 ARM64 reliability ([zed#36798](https://github.com/zed-industries/zed/issues/36798)) |
| Windows 10 | <1809 (pre-ConPTY) | any | Unsupported - ConPTY is required for PTY-based terminals |
| Windows Server | 2019 / 2022 | x86_64 | Untested, unsupported in v1 |

PaneFlow renders via the GPUI DirectX 11 backend (landed upstream
2025-07-30, PR [zed#34374](https://github.com/zed-industries/zed/pull/34374)).
A DirectX 11 feature-level-10+ capable GPU driver is required. Most
Windows 10 1809+ installs satisfy this; see the
[NoSupportedDeviceFound](#gpu-nosupporteddevicefound-on-older-drivers) risk
below for the exception.

---

## 2. Installation

### Direct MSI download

Download `paneflow-<version>-x86_64-pc-windows-msvc.msi` from the
[latest GitHub Release](https://github.com/arthjean/paneflow/releases/latest)
and double-click to install. The installer is signed via Azure
Artifact Signing under the **Strivex** certificate profile -
SmartScreen may show "Windows protected your PC" with publisher
**Strivex** (not "Unknown Publisher"). Click `More info` →
`Run anyway` to proceed. Reputation accumulates over the first
few weeks after a fresh cert profile is issued, so prompts should
become less frequent for that publisher identity.

Uninstall from `Settings → Apps → Installed apps → PaneFlow →
Uninstall`. The installer removes `%ProgramFiles%\PaneFlow\` but
leaves `%APPDATA%\paneflow\paneflow.json` and related user config
untouched - idiomatic Windows uninstall behaviour.

### winget status

A `winget` package is not the authoritative install path for the current Windows
release. Use the signed MSI above until `ArthurDev44.PaneFlow` is accepted in
`microsoft/winget-pkgs` and visible through `winget search`.

### Building from source

Same instructions as the [README Build from source](../README.md#build-from-source)
section, with `--target x86_64-pc-windows-msvc`. WiX Toolset 3.14
must be installed locally for MSI production; see
[`scripts/sign-windows.ps1`](../scripts/sign-windows.ps1) if you want
to reproduce the signed MSI end-to-end.

---

## 3. Known limitations (v1)

These are PaneFlow's own limitations on Windows - distinct from the
upstream risks in §4 below.

- **Service labels can be less rich on Windows.** The Windows scanner
  discovers LISTEN ports with `GetExtendedTcpTable` and attributes them
  to each pane's process subtree. Some JavaScript dev servers may still
  appear as raw port chips rather than branded Vite/Next.js-style labels
  when Windows exposes only the socket owner's executable name.
  Workaround: open the raw port chip or use the printed dev-server URL.

- **`cmd.exe` does not emit OSC 7, so CWD-aware features are
  degraded in `cmd.exe` panes.** PaneFlow tracks each pane's current
  working directory by parsing the OSC 7 escape sequence the shell
  emits on every `cd`. Legacy `cmd.exe` does not emit OSC 7;
  PowerShell 5.1, PowerShell 7, and Git Bash do when configured with
  PaneFlow's setup scripts. The Git Bash preset resolves Git for
  Windows before Windows' built-in WSL `bash.exe` launcher.
  Workaround: use PowerShell (`pwsh` or `powershell.exe`) or Git Bash
  for panes where you rely on
  split-from-same-cwd or workspace-directory hints.

- **Integrated WSL shells use interactive non-login startup.** When
  `default_shell` points to `wsl.exe` and shell integration is enabled,
  PaneFlow resolves the default Linux Bash, Zsh, or Fish and loads its
  interactive startup file (`.bashrc`, `.zshrc`, or `config.fish`). This
  matches PaneFlow's native shell behavior, but does not evaluate login-only
  files such as `.profile`, `.bash_profile`, or `.zprofile`. Put environment
  required by interactive terminals in the corresponding interactive startup
  file, or disable shell integration to keep the unmodified `wsl.exe` launch.

- **CJK IME input is fragile on some Windows / GPUI combinations.**
  See [IME CJK panic](#ime-cjk-panic) in §4. Basic CJK input works
  in PaneFlow v1 smoke testing, but composition-heavy workflows
  (particularly Chinese IME with dead keys) can trigger rare
  upstream panics. If you rely on CJK input daily, file a report
  via the Windows bug template so we can upstream the repro.

- **`cwd_now()` via `NtQueryInformationProcess` returns `None` in
  v1.** The Windows implementation of runtime CWD detection (used
  as a fallback when OSC 7 isn't available) returns `None` in v1
  and defers to OSC 7. Tracked as an Out-of-Scope item in the PRD.

---

## 4. Known upstream risks

The five risks below are defects in PaneFlow's upstream dependencies
(GPUI, portable-pty, Windows ConPTY). PaneFlow does not own
the fix; we monitor each issue and ship v1 with documented mitigation
so users know what to expect.

### IME CJK panic

- **Upstream:** [`zed-industries/zed#12563`](https://github.com/zed-industries/zed/issues/12563)
- **Severity:** functional (crash)
- **Description:** Composing certain CJK input sequences with a
  Windows IME can, in rare cases, trigger an assertion in the GPUI
  Windows IME handler and crash the window. The upstream issue is
  marked closed but remains fragile in the DX11 backend.
- **v1 workaround:** If the crash reproduces with your IME,
  file the exact sequence via the Windows bug template and switch
  to typing via an IME that buffers the full token before
  committing (avoid commit-per-keystroke IMEs until a GPUI fix
  lands). Basic CJK input works in smoke tests.

### ConPTY Ctrl+C signal propagation

- **Upstream:** the Windows ConPTY API itself, documented from a
  terminal-emulator perspective in
  [`alacritty/alacritty#3075`](https://github.com/alacritty/alacritty/issues/3075)
- **Severity:** functional (may propagate unexpectedly)
- **Description:** The Windows ConPTY API does not cleanly
  distinguish "user pressed Ctrl+C in this terminal" from "send
  SIGINT to the foreground process group". In some shell
  configurations, Ctrl+C can propagate to parent processes in a
  way POSIX users don't expect.
- **v1 workaround:** For cases where Ctrl+C misbehaves,
  use the shell's built-in abort verb (`pwsh`'s `break` from inside
  a loop; `cmd.exe`'s `Break` menu). A real fix depends on ConPTY
  itself, not on Paneflow.

### RDP initialization broken

- **Upstream:** [`zed-industries/zed#26692`](https://github.com/zed-industries/zed/issues/26692)
- **Severity:** blocker (on-RDP only)
- **Description:** Launching PaneFlow inside an active Remote
  Desktop Protocol session can fail to initialize the DX11 device
  context - the window comes up but rendering is broken or the
  process exits with a device-creation error.
- **v1 workaround:** Use PaneFlow on a local session; avoid
  launching from inside an RDP session. If RDP is unavoidable,
  the Windows built-in terminal (`wt.exe`) remains available as a
  fallback.

### Devcontainer / WSL2 freeze

- **Upstream:** [`zed-industries/zed#49072`](https://github.com/zed-industries/zed/issues/49072)
- **Severity:** functional (freeze on launch from devcontainer)
- **Description:** Launching PaneFlow from inside a VS Code
  devcontainer or a WSL2 shell (on the Windows host, not the Linux
  side) can cause the GPUI event loop to freeze before the first
  frame. The process remains responsive to Task Manager but never
  renders.
- **v1 workaround:** Launch PaneFlow from a regular Windows shell
  (Start Menu, Run dialog, or a native Windows terminal). Do not
  nest through a devcontainer shell. WSL2 *inside* PaneFlow (as a
  shell choice for a pane) works - the restriction is only on
  launching the app itself from a WSL2 context.

### GPU `NoSupportedDeviceFound` on older drivers

- **Upstream:** [`zed-industries/zed#28683`](https://github.com/zed-industries/zed/issues/28683)
- **Severity:** blocker (install-time)
- **Description:** On machines with DirectX 11 drivers older than
  Windows 10 1909-ish, the GPUI DX11 backend fails to find a
  supported device and PaneFlow exits immediately with a
  `NoSupportedDeviceFound` error. This is mostly seen on unpatched
  Windows 10 images and on older integrated Intel GPUs with
  stock Windows-Update drivers.
- **v1 workaround:** Update the graphics driver through Windows
  Update OR the GPU vendor's site (Intel, NVIDIA, AMD). If the
  error persists after a driver update, file a Windows bug
  report with the output of `dxdiag` attached and we'll upstream
  the hardware combination.

---

## 5. Reporting issues

Windows bugs use a dedicated issue template so the first responders
get the runtime context they need without a back-and-forth:

- **Template:** [`Windows bug report`](https://github.com/arthjean/paneflow/issues/new?template=windows-bug-report.md)
  (`.github/ISSUE_TEMPLATE/windows-bug-report.md`). Includes fields
  for Windows version + build, architecture, install format, and
  an embedded logs block.
- **Include logs.** Launch PaneFlow from a PowerShell prompt with:
  ```powershell
  $env:RUST_LOG = "info"
  & "C:\Program Files\PaneFlow\paneflow.exe"
  ```
  Paste the stderr output covering the bug window. If PaneFlow
  crashes immediately on launch, re-run with `$env:RUST_BACKTRACE=1`
  and attach the backtrace.
- **Triage routing.** Reports land in the general triage queue and
  are labeled `windows`. Upstream-risk-class bugs (§4) are relabeled
  `upstream/<zed|conpty>` and cross-referenced to the tracking
  issue; fixes ride along with the relevant dependency bump.

For the smoke-test checklist run on every release, see
[`WINDOWS-SMOKE-TEST.md`](WINDOWS-SMOKE-TEST.md) (US-021) - sibling
runbook in this same `docs/` directory.
