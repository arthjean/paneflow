# Windows browser host qualification

The Windows browser target is `x86_64-pc-windows-msvc`. EP-001 records the
signed CEF Windows64 archive at the exact CEF/Chromium/cef-rs pins shared with
the Linux contract. The target remains development-only and its native
qualification is control-only on Windows 11; GPU, presentation and public
availability remain owned by EP-002 and later epics.

The declared operating-system floor is Windows 10 version 1809, build 17763,
with Windows 11 x64 included. Windows 11 build 26200 has executed the pinned
bootstrap, sandbox entry point and control contract. This does not claim
Windows 10 execution, GPU presentation, browser-page qualification or release
signing.

## Runtime and client staging

Fetch is explicit and verifies the archive size, SHA-1, SHA-256, per-file
hashes and the complete extracted-tree digest:

```powershell
python scripts/fetch-browser.py --target x86_64-pc-windows-msvc
```

The fetch script never downloads from `build.rs` or on first browser use. The
signed runtime archive has no CEF headers or import library. The native client
build therefore also needs the standard CEF development archive at the same
`version_full` and `abi_hash`, while staging always copies the signed runtime.
Both `PANEFLOW_CEF_ROOT` and `CEF_PATH` point to the development root during
compilation:

```powershell
pwsh scripts/build-browser-host.ps1 `
  -Runtime native/browser/prebuilt/x86_64-pc-windows-msvc/<archive-sha256> `
  -CefDevelopmentRoot path/to/cef_binary_<same-version>_windows64 `
  -Bootstrap path/to/cef_binary_<same-version>_windows64/Release/bootstrap.exe
```

The script validates PE headers, checks the `RunWinMain` export when `dumpbin`
is available, records Authenticode status and writes a staging receipt. A
release build must pass `-RequireSignature`.

## Process contract

Paneflow launches the bootstrap executable, never `libcef.dll` directly. The
bootstrap loads the client DLL and invokes its `RunWinMain` export with the
CEF `sandbox_info` pointer. The `browser_subprocess_path` setting is not used
to bypass this bootstrap contract. Renderer descendants are assigned to a
Windows Job Object with `KILL_ON_JOB_CLOSE`.

Control traffic uses a per-host named pipe under `\\.\pipe\paneflow-browser-*`.
The pipe descriptor grants full access only to SYSTEM, built-in administrators
and the object owner. The host receives the workspace/session owner and the
profile path through the control bootstrap. Profile roots and the owner lock
remain inside Paneflow's per-user data directory.

The supervisor validates the manifest stamp, pinned Windows runtime files,
bootstrap PE identity, capability version and reported process identity before
the dock receives `Ready`. Any missing archive, checksum, ABI, bootstrap or
runtime contract leaves Browser unavailable and preserves terminal behavior.

## Control-only qualification receipt

The local qualification harness signs its private copy of `chrome_elf.dll`,
the bootstrap and the client with one short-lived test certificate. This is
needed because CEF requires those three binaries to share a signer, and it
must never be confused with release signing. The harness validates the real
`RunWinMain` path, `sandbox_info`, contract version, control-only presentation
and a create/close protocol round trip:

```powershell
pwsh scripts/browser-qualification/windows-bootstrap.ps1 `
  -Runtime native/browser/prebuilt/x86_64-pc-windows-msvc/<archive-sha256> `
  -Bootstrap target/windows-browser-bundle-final/lib/paneflow/paneflow-browser-host.exe `
  -ClientDll target/windows-browser-bundle-final/lib/paneflow/paneflow-browser-host.dll `
  -Output target/windows-browser-qualification `
  -SignWithTestCertificate
```

The resulting receipt is
`target/windows-browser-qualification/windows-bootstrap-qualification.json`.
The process is reclaimed by the bounded cleanup fallback after the control
round trip; the receipt records that exit status explicitly.

## M1 capture campaign

`scripts/browser-qualification/windows-capture.ps1` records one M1 repetition.
Configuration A runs Paneflow with four replay terminals and no browser, B runs
`paneflow browser-prototype` on the same fixture without the dock, and C runs the
integrated dock. Each repetition keeps the M1 10 second warm-up and 60 second
measurement, writes `capture.json`, `presentmon.csv`, `application.jsonl` and
`resources.jsonl`, and refuses to certify when PresentMon exits without a CSV.

PresentMon runs unelevated with the Performance Log Users right, but it needs a
free ETW session name. A killed capture used to leave `Paneflow-M1-<pid>`
running, and the leaked sessions then starved the next capture until it lost
every present event and wrote no CSV. The harness reclaims stale
`Paneflow-M1-*` sessions before starting, passes `--stop_existing_session` and
stops its own session in the cleanup path; `capture.json` records the reclaimed
names.

`capture.json` also carries `os_identity`. `Get-ComputerInfo` and the registry
`ProductName` value still report "Windows 10 Pro" on Windows 11, so the identity
comes from the CIM caption, `CurrentBuild`, `UBR` and `DisplayVersion`, and a
build at or above 22000 is Windows 11. `displays.scales` records the effective
per-monitor scale from `GetDpiForMonitor`.

`scripts/browser-qualification/windows-m1-campaign.ps1` runs the whole protocol:
it serves the fixture, captures every configuration five times, analyzes each
repetition and writes `campaign.json` next to the comparison report.

```powershell
pwsh scripts/browser-qualification/windows-m1-campaign.ps1 `
  -Output target/ep002-m1-r0 -Bundle target/windows-browser-bundle-ep002-final
```

`bun scripts/browser-qualification/windows-analysis.mjs <directory>` correlates
one capture. `bun scripts/browser-qualification/windows-m1-compare.mjs
report.json <directories...>` aggregates the repetitions into the median and the
worst repetition per configuration, then the observed C minus B presentation
delta and the C minus A terminal and memory deltas. It refuses captures produced
by different binaries, hosts or runtimes, and reports observations, never a
budget verdict.

## Keyboard, IME and DPI evidence

`paneflow browser-prototype` carries the Windows input steps. It reads the same
staging environment as the capture harness, so point it at a signed staging and
serve the fixture first with
`bun scripts/browser-qualification.mjs serve 18762`:

```powershell
$staging = 'target/ep002-staging-final'
$env:PANEFLOW_BROWSER_HOST = "$staging/browser/Release/paneflow-browser-host.exe"
$env:PANEFLOW_CEF_ROOT = "$staging/browser"
$env:PANEFLOW_BROWSER_QUALIFICATION_SHA256 = python -c "import importlib.util,pathlib; s=importlib.util.spec_from_file_location('bp','scripts/browser-package.py'); m=importlib.util.module_from_spec(s); s.loader.exec_module(m); print(m.runtime_digest(pathlib.Path('$staging/browser')))"
$env:PANEFLOW_BROWSER_STAGE_ROOT = 'target/ep002-input-evidence/staging'
target\release\paneflow.exe browser-prototype --url http://127.0.0.1:18762/ime `
  --scenario input,wheel,drag,resize,scale,dpi,ime,cancel `
  --dpi-transitions 150,200 --log C:\absolute\prototype.jsonl
```

`PANEFLOW_BROWSER_QUALIFICATION_SHA256` pins the digest of a locally
re-signed staging tree instead of the manifest one, so it replaces the per-file
verification of the runtime. The `browser-prototype` verb reads it directly
because it is an explicit operator command. A dock page reads it only inside an
isolated qualification session, that is when `PANEFLOW_M1_LOG` and an absolute
`PANEFLOW_M1_STATE_ROOT` are both set; an ordinary session keeps the manifest
check whatever the variable holds.

`scale` moves the real window to the next monitor and records both monitors.
`dpi` changes the display scale of the monitor holding the window through the
DisplayConfig DPI packets, waits for the window scale factor to follow, records
the viewport, the presented geometry and the IME candidate rectangle at each
scale, then restores the previous scale. It refuses a primary monitor, and the
restore also runs on failure and on shutdown. `--dpi-transitions` is required
for any display change: without it the step records `dpi_skipped` and touches no
display configuration.

`ime` runs the ASCII, accented, Japanese candidate, astral, cancelled and
finished compositions against the fixture input focused with Tab, and records
the candidate rectangles reported by CEF for each case. `cancel` covers focus
loss, capture loss, an input addressed to a stale document generation, which the
controller refuses with `StaleGeneration`, and the viewport coordinates observed
after the last geometry change. The `machine` event at the start of the log
carries the same OS identity fields as `capture.json`.

## Qualification still required

EP-002 owns the first browser-page execution and D3D/external-surface path.
The following evidence must be collected before changing the target to a
qualified availability:

- Windows 10 1809 and Windows 11 x64 bootstrap and sandbox harness results.
- Authenticode and client export verification on the release artifacts.
- Unknown-DLL, wrong-ABI, root-in-use, dead-host and refused-peer tests.
- Job Object descendant cleanup and profile ACL checks under a normal user.
- Native browser creation, navigation, renderer crash recovery and presentation
  smoke results.
