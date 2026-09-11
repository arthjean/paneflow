# Using the Windows Browser surface

This page describes what the Browser surface does on Windows, where it keeps its
data, and which decisions stay with the person at the keyboard. It documents the
behavior implemented on `x86_64-pc-windows-msvc`. It is not a support promise:
the Windows human release verdict is `NOT_QUALIFIED` and the agent verdict is
`NOT_QUALIFIED` in
[windows-qualification-contract.toml](../../native/browser/windows-qualification-contract.toml),
so the capability is exposed only as a development target.

## Installation

The Windows package is an MSI that installs to `%ProgramFiles%\PaneFlow\`
([packaging/wix/main.wxs](../../packaging/wix/main.wxs)). When the browser
variant is built, it adds two payload trees under that prefix:

| Path | Content |
|---|---|
| `lib\paneflow\paneflow-browser-host.exe` | The sandbox bootstrap process |
| `lib\paneflow\paneflow-browser-host.dll` | The client that CEF calls through `RunWinMain` |
| `lib\paneflow\browser\Release\` | `libcef.dll`, the ICU data and the CEF binaries |
| `lib\paneflow\browser\Resources\` | The `.pak` resources and every locale |
| `lib\paneflow\browser\verified-manifest.sha256` | The digest that ties the installed runtime to its manifest |

Discovery is prefix relative: PaneFlow resolves the payload from the directory
holding `paneflow.exe`, never from a checkout, an installed Chrome or a download
at first use ([install_windows.rs](../../src-app/src/browser/install_windows.rs)).
A partial payload, a stamp written by another manifest or a client DLL without a
real `RunWinMain` export keeps the browser absent and reports a repair reason
instead of opening a page.

No CEF process starts and `libcef.dll` is not loaded until a Browser page is
activated. Installing the payload does not by itself run a browser.

## The page journey

Browser is a surface type of the Agents dock. It appears in the surface picker
and in the dock `+` menu, and the address bar accepts a URL directly. Each tab
belongs to one Agents session, keeps that owner when parked, and its page is not
recreated when you change session.

The toolbar carries back, forward, reload or stop, the address field, inspect
and an overflow menu. Below 520 logical pixels of dock width, inspect and the
secondary actions move into that menu while back, forward, reload or stop, the
address and the menu stay reachable.

Default Windows keys, all remappable in the shortcut registry
([defaults.rs](../../src-app/src/keybindings/defaults.rs)), apply only when the
Browser chrome or its document holds focus:

| Key | Action |
|---|---|
| `Ctrl-L` | Focus and select the address |
| `Ctrl-T` | New empty Browser tab in this session |
| `Ctrl-W` | Close the current Browser, or the inspector first when it has focus |
| `Ctrl-R` / `Ctrl-Shift-R` | Reload, reload ignoring the cache of the current request |
| `Alt-Left` / `Alt-Right` | History back and forward |
| `Ctrl-F` | Find in page |
| `Ctrl-=` / `Ctrl--` / `Ctrl-0` | Zoom in, out, reset |
| `F6` / `Shift-F6` | Cycle address, document, inspector, previously active terminal |
| `Escape` | Cancel the composition or overlay, leave fullscreen, close find, otherwise stop loading |

A key aimed at the page never reaches a PTY, and a late title, load or popup
event never moves the focus.

## Errors

Browser failures stay inside the dock and never stop a terminal. A page that
cannot load shows an error state in the viewport with `Reload` and
`Open externally` ([view.rs](../../src-app/src/browser/view.rs)). A stopped local
server produces that state; it does not delete the tab.

Three other notices can appear above the page: `The browser is unavailable` when
the runtime is missing or refused, `The browser is unavailable on this platform`
when the target has no backend, and `Acceleration unavailable: <reason>` when the
presentation path cannot be established. A host process death is reported in the
dock, and the supervisor restarts the host on the next activation.

## Local data

Browser data lives under the PaneFlow data directory, which on Windows resolves
to `%LOCALAPPDATA%\paneflow\browser` for a release build and
`%LOCALAPPDATA%\paneflow-dev\browser` for a debug build
([runtime_paths.rs](../../src-app/src/runtime_paths.rs),
[profile.rs](../../src-app/src/browser/profile.rs)).

One profile belongs to one workspace. Worktrees and sessions of the same
workspace share it; a separately added workspace gets its own profile even when
it points at the same local URL. Cookies and storage of one workspace are not
readable from another. Nothing is imported from an external Chrome or Helium
profile.

The profile root is created with a protected ACL that grants SYSTEM,
administrators and the owner, and it holds `owner.lock`. A second PaneFlow
instance cannot open a profile that is already locked, and a runtime whose
digest does not match its manifest stamp is refused rather than run.

Closing a Browser tab destroys its web session after `beforeunload` is handled.
Restoring a saved layout brings the tabs back under their sessions in the
`Dormant` state until you select them.

## Permissions and OS dialogs

Permission requests, certificate decisions, popups, downloads, file pickers and
external protocol handoffs are human decisions. They use the native Windows
dialogs through the shared policy
([permissions.rs](../../crates/paneflow-browser-host/src/linux/permissions.rs),
[windows.rs](../../crates/paneflow-browser-host/src/windows.rs)). The origin is
re-parsed and restricted to `https` or loopback `http`, and the request mask is
limited to the capabilities the policy allows. A request that becomes stale,
because the document navigated, the renderer crashed or the page closed, is
canceled and grants nothing. A downloaded file is never executed automatically.

## DevTools

`Inspect` in the Browser menu opens DevTools through the public CEF API as a
secondary presentation attached to the inspected page
([devtools.rs](../../src-app/src/browser/view/devtools.rs)). No public CDP port
is opened. The page and inspector ratio is remembered for the current session
only, and DevTools does not reopen at startup. When the embedded inspector is
unavailable, the dock reports it instead of falling back to a remote port.

## Agent access

Agent access to Browser pages is a separate contract, disabled per workspace by
default. `Allow agent to read pages` and the interact entry in the Browser menu
are human actions; no agent endpoint can change them. `Select page context`
prepares a visible context in the owning session composer and never submits it
automatically. See [agent-tools.md](agent-tools.md).

## What is not qualified yet

The Windows matrix, its security cases and its budgets are declared in
[windows-qualification-contract.toml](../../native/browser/windows-qualification-contract.toml)
and validated by:

```sh
python3 scripts/verify-windows-qualification-contract.py --contract native/browser/windows-qualification-contract.toml
```

Until every row of that matrix is measured on real hardware, the distribution
manifest keeps `availability = "development"` for this target and the verifier
rejects any promotion beyond it.
