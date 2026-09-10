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

## Installed layout and MSI packaging

The MSI installs PaneFlow into `%ProgramFiles%\PaneFlow\`, which a standard user
cannot modify. The browser payload keeps the prefix-relative layout the other
platforms use, so one resolution rule covers every target:

```
%ProgramFiles%\PaneFlow\paneflow.exe
%ProgramFiles%\PaneFlow\lib\paneflow\paneflow-browser-host.exe
%ProgramFiles%\PaneFlow\lib\paneflow\paneflow-browser-host.dll
%ProgramFiles%\PaneFlow\lib\paneflow\browser\Release\...
%ProgramFiles%\PaneFlow\lib\paneflow\browser\Release\locales\...
%ProgramFiles%\PaneFlow\lib\paneflow\browser\verified-manifest.sha256
%ProgramFiles%\PaneFlow\share\doc\paneflow\BROWSER_THIRD_PARTY_NOTICES.md
%ProgramFiles%\PaneFlow\share\doc\paneflow\browser-sbom.json
```

`install_windows.rs` resolves the runtime from the real executable path, so the
installed layout needs neither a developer checkout nor any download on first
use. `PANEFLOW_CEF_ROOT` and `PANEFLOW_BROWSER_HOST` still override it for
development builds.

The installer definition stays one file. `packaging/wix/main.wxs` carries a
`BrowserRuntimeFeature` guarded by `<?ifdef BrowserStage ?>`: without
`-dBrowserStage=<staged prefix>` `cargo wix` produces exactly the terminal-only
MSI it has always produced, and the CEF runtime is neither referenced nor
fetched. The feature references a `BrowserRuntime` component group generated
from the staged payload, one component and one GUID per file, so the file list is
never maintained by hand:

```powershell
python scripts/fetch-browser.py --target x86_64-pc-windows-msvc --verify-only
pwsh scripts/build-browser-host.ps1 -Runtime <runtime> -CefDevelopmentRoot <dev> -Bootstrap <dev>\Release\bootstrap.exe
python scripts/browser-package.py --target x86_64-pc-windows-msvc stage
pwsh scripts/browser-msi.ps1 -Stage target/browser-stage -Sign
```

`browser-msi.ps1` restamps the shipped SBOM for the `msi` format, regenerates
the plan, verifies it, signs the two components
PaneFlow itself produces (`paneflow-browser-host.exe` and its client DLL) through
`scripts/sign-windows.ps1`, re-verifies against the signed bytes, runs
`cargo wix` with the staged prefix and signs the MSI. Both WiX sources are
passed with `--include`: cargo-wix 0.3.9 ignores the `wxs` key in
`[package.metadata.wix]` and only auto-discovers `src-app/wix/*.wxs`, which does
not exist, so `packaging/wix/main.wxs` and the generated fragment are named
explicitly, exactly as the release workflow already does for the terminal-only
MSI. The Azure Artifact Signing
variables listed in [windows-signing.md](windows-signing.md) come from the
maintainer environment; none of them lives in the repository. Without `-Sign` the
script stops after the plan, prints `distributable: false` and never emits an
unsigned installer. The CEF runtime files are copied byte for byte from the
pinned archive and are not re-signed: CEF signatures stay intact, and
`verify_runtime` re-checks every manifest digest before the host may load
`libcef.dll`.

## Package verification

```powershell
python scripts/browser-package.py --target x86_64-pc-windows-msvc verify --format msi --prefix target/browser-stage
python scripts/browser-package.test.py
```

`verify` exits non-zero on the first failing check and prints a JSON verdict.
Beyond the layout, stamp, per-file digests, tree digest, bootstrap and client PE
contracts, runtime metadata, ABI, credits, license, notices, checkout
independence and the NFR-13 installed budget, the Windows path checks:

- `runtime_architecture`: every bundled PE is an x86_64 image.
- `runtime_imports`: every imported module is bundled, a Windows system DLL, an
  `api-ms-win-*` or `ext-ms-win-*` apiset, or the Microsoft C runtime. Nothing
  resolves from the current directory or from a user-writable directory on
  `PATH`. The only redistributable import of the current pin is
  `vcruntime140.dll`, the dependency `paneflow.exe` already has.
- `bundle_paths`: no staged file escapes the bundle prefix.
- `runtime_codecs`: the codec table in the manifest matches what the runtime
  really contains.
- `msi_plan`, `msi_fragment`, `msi_signed_components`: the generated plan covers
  every packaged file with the digest it will ship, the WiX fragment declares the
  `BrowserRuntime` group, and the components that need a PaneFlow signature are
  exactly the bootstrap and its client DLL.

`scripts/browser-package.test.py` runs the plan against a synthetic bundle and is
part of the Linux test job, so a foreign import, a wrong-architecture image, a
payload edited after planning, a missing plan or a codec table that drifts from
the runtime all fail without needing a Windows machine.

The host DLL search path is bounded at launch:
`supervisor_windows.rs::host_search_path` gives the child process the staged
`Release` directory followed by the Windows system directories only. The user
`PATH` never contributes, and the working directory is the runtime directory
itself.

## Codecs and redistribution

The signed Windows archive is stripped, so the `ff_*_decoder` symbol names the
Linux probe reads are absent from `libcef.dll`. FFmpeg keeps the
`AVCodec.long_name` string inside each decoder struct, which is compiled in only
when that decoder is enabled, so the long name is the measurable signal on this
artifact. Measured on the current pin, H.264, HEVC, AAC, MP3, FLAC and Vorbis are
all present. The manifest records those measured values, so the SBOM reports
`restricted_codecs = [aac, h264, hevc]` and `redistribution = review_required`.
That is a finding for the release verdict, not a blocker for development builds;
no distribution decision is taken here.

The manifest declares no GN hardening table for this target. The signed archive
carries no build-configuration record, and none is inferred from the Linux
audit.

## Update, rollback and profile coupling

The application and the runtime are replaced together by one MSI. There is no
independent CEF updater and none is to be added.

- Before staging an update, `browser::update::plan` decides on the real
  installation site. A live browser host blocks the replacement, because Windows
  cannot overwrite a runtime file the host holds. An installation drive without
  room for the declared payload blocks it too. The installed version and its
  profiles are kept in both cases.
- A runtime whose `verified-manifest.sha256` does not match the manifest digest
  the application was built from is never mixed into a working installation:
  `install::classify` reports it unusable, `BrowserAuthority::detect` drops
  availability to absent, and the dock states that the installation has to be
  repaired. The update plan reports the same state as a resumable installation,
  which is what a reboot-interrupted or partial MSI leaves behind.
- The relay that runs `msiexec` waits for the PaneFlow process to exit first, and
  every browser host is assigned to a Job Object with `KILL_ON_JOB_CLOSE`, so no
  descendant still holds a runtime file when the replacement starts.
- A profile records the engine that wrote it. A profile written by a newer engine
  is refused with "This data needs a newer browser version" instead of being
  reopened blindly; an older one is migrated and keeps a `migrated_from` field.
  The marker is written through a temporary file and a rename, so a failed write
  leaves the previous marker intact. No downgrade rewrites profile data.

`browser::update::plan` carries fixtures for the retained-file,
insufficient-space, interrupted and resumed cases. The real native update, the
standard-user installation and the loaded-DLL verification on an installed
machine belong to US-013.

## Engine security maintenance

This is the NFR-12 contract for the pinned Windows runtime. It mirrors the Linux
one because the CVE source is the same.

- Watch the Chrome stable-channel security posts and the CEF release
  announcements for the branch the manifest pins. Chromium CVEs reach PaneFlow
  through CEF, so the CEF branch build is the artifact to wait for, not the
  Chrome version number.
- Triage an applicable critical vulnerability within 48 hours of learning about
  it: decide whether the pinned branch is affected, and record the decision with
  its evidence.
- Target a corrected package within 7 days of a distributable CEF build being
  available. When upstream has no build yet, record that unavailability and the
  mitigation in the same place, within the same delay. Never publish a version
  claimed as corrected when the fix is not in the shipped bytes.
- Re-pin with `scripts/fetch-browser.py`, then re-run the packaging verification,
  the codec measurement, the SBOM and the notices. A re-pin is a new
  qualification of the runtime, not a version-string edit. `cargo deny` audits
  Cargo dependencies only and never covers this runtime.
- Changing the manifest changes its digest, so every staged prefix has to be
  re-staged and the previous `verified-manifest.sha256` stamps stop matching by
  design. That is the mechanism that refuses a version mix, not a regression.

## Accessibility and web interaction bridges

The Windows client exposes the same handlers the Linux host does, so the shared
dock, permission and accessibility controllers are reused rather than copied:

- `set_accessibility_state(ENABLED)` on creation plus the CEF accessibility
  handler forward tree and location updates to `browser::accessibility`, which
  builds the AccessKit subtree GPUI publishes to the platform. Updates carry
  their document identity, so a replaced document drops stale nodes instead of
  merging them, and an oversized or malformed update marks the tree unavailable
  rather than fabricating content.
- Permissions, script dialogs, file pickers, download destinations and external
  protocols run through the shared P1 controllers. Every decision is re-checked
  against the origin, profile and document generation when the human answer
  comes back, and a strict certificate policy is enforced on the request context
  before the page loads.
- File selection and download destinations use the GPUI native Windows dialogs
  and `dirs::download_dir`; the host never accepts an arbitrary path, and no
  download is opened or executed automatically.
- Drag-and-drop file upload and the renderer clipboard round trip stay Linux-only
  for now. Windows copy and cut go through the Chromium clipboard path, and the
  asynchronous clipboard read is governed by the shared permission policy.
- Agent console and network diagnostics are not emitted by the Windows host yet;
  they belong to the agent-tools story.

## Qualification still required

EP-002 owns the first browser-page execution and the D3D external-surface path;
EP-003 prepares the remaining OS integrations and the package. The following
evidence has to be collected on real hardware before this target changes to a
qualified availability:

- Windows 10 1809 and Windows 11 x64 bootstrap and sandbox harness results.
- Authenticode and client export verification on the release artifacts, plus a
  real signed MSI, a standard-user installation and a loaded-DLL check on the
  installed machine (US-013).
- Unknown-DLL, wrong-ABI, root-in-use, dead-host and refused-peer tests.
- Job Object descendant cleanup and profile ACL checks under a normal user.
- Native browser creation, navigation, renderer crash recovery and presentation
  smoke results.
- A Narrator pass over the document and the dock chrome, and the native
  permission, download, file-picker and external-protocol dialogs driven by a
  human (US-013).
- A real interrupted-update and resume cycle on an installed machine (US-013).

The accessibility bridge, the permission and file paths, the bundle plan and the
update plan implemented by EP-003 are proven by contract tests reached from their
real entry points on a development host. None of them claims a native Windows
observation.
