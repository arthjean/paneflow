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

The cleanup path deletes that test certificate from `Cert:\CurrentUser\My` and
`Cert:\CurrentUser\Root` unless `-KeepTestCertificate` is passed. The binaries
stay signed, so any tree signed with it becomes unloadable the moment the script
returns: the CEF bootstrap aborts with `Failed <bootstrap> certificate checks:
Certificate 0: WinVerifyTrust failed (-2146762487)` in the profile's
`host.stderr`, the control pipe never opens, and the dock reports a lost host.
Pass `-KeepTestCertificate` when the signed tree has to outlive the harness run,
and sign a staging meant for later runs with `windows-sign-staging.ps1`.

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
repetition and writes `campaign.json` next to the comparison report. Leave the
machine alone while it runs: a capture whose window loses the foreground is
refused, for the reason given in "Foreground ownership is part of the protocol".

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
`bun scripts/browser-qualification.mjs serve 18762`.

`windows-sign-staging.ps1` produces that signed staging. It signs
`chrome_elf.dll`, the bootstrap and the client with one code-signing certificate
that it keeps in `Cert:\CurrentUser\Root`, reuses that certificate while it
remains valid instead of minting one per run, refuses any signature the machine
does not trust back, and prints the runtime digest to pin. It needs Windows
PowerShell for the certificate cmdlets:

```powershell
powershell -NoProfile -File scripts/browser-qualification/windows-sign-staging.ps1 `
  -Runtime target/ep002-staging-final/browser `
  -Bootstrap target/ep002-staging-final/browser/Release/paneflow-browser-host.exe
```

Re-signing `chrome_elf.dll` changes the runtime tree, so
`PANEFLOW_BROWSER_QUALIFICATION_SHA256` has to be taken from that run, either
from the printed value or from the digest helper below:

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
- Agent console and network diagnostics are emitted by the Windows client through
  the same `agent_console` and `agent_network` shapes the Linux host uses, and an
  agent navigation terminalizes at the main-document commit through
  `agent_navigation_committed`, `agent_navigation_failed` and
  `agent_navigation_cancelled`. The Windows control thread shares its
  `Controller` with the CEF UI thread so the commit produces the new document
  generation the application requires. No native Windows agent session has been
  observed yet, so the agent verdict stays `NOT_QUALIFIED`.

## S1-W qualification matrix and release verdict

The Windows verdict is declared in
[windows-qualification-contract.toml](../../native/browser/windows-qualification-contract.toml)
and validated at every CI run on the release-packaging job:

```sh
python3 scripts/verify-windows-qualification-contract.py --contract native/browser/windows-qualification-contract.toml
python3 scripts/verify-windows-qualification-contract.test.py
```

The contract carries the seven S1-W rows, SEC-01 through SEC-12, the ten NFR
budgets this release gates on, and the documents the report depends on. Row
statuses use the plan vocabulary: `NOT_EXECUTED`, `WORKS_NOT_MEASURED`,
`WORKS_MEASURED` and `FAILED`. The verifier refuses a row that claims execution
without an evidence path and a machine identity, a security case promoted past
its proof kind, a budget declared measured without evidence, and a document that
does not resolve to a written file. It also cross-checks the distribution
manifest: while `human_release` is `NOT_QUALIFIED`, `availability` for
`x86_64-pc-windows-msvc` may only be `absent` or `development`.

The application enforces the same ceiling at its own entry point.
`browser::install_windows::declared_availability`, the function
`BrowserAuthority::detect` calls on Windows, clamps whatever the manifest
declares to what the contract's verdicts allow, so a promoted manifest alone
never exposes a qualified capability.

Current state: `human_release = NOT_QUALIFIED`, `agent_release = NOT_QUALIFIED`.
The `first-smoke` row is executed as `WORKS_NOT_MEASURED` on Windows 11 build
26200 UBR 9278 with an NVIDIA GeForce RTX 4070 Ti SUPER, `minimum-product` is
`WAIVED_BY_OWNER`, and every other row is `NOT_EXECUTED` and names what is
missing. No Linux or macOS result is accepted
as evidence for this target, and the contract records that refusal explicitly.

### The Windows 10 1809 waiver

The owner accepted, on 2026-09-11, that the declared floor rests on code and
tests rather than on a native Windows 10 1809 run. The `minimum-product` row
carries status `WAIVED_BY_OWNER` with the decider, the date, the reason, the
code that enforces the floor and the residual risk. The verifier refuses a
waiver that omits any of those fields and refuses one that presents native
evidence it does not have.

What now enforces the floor:
`browser::install_windows::floor_verdict` reads `minimum_windows` from the
manifest, reads `CurrentBuildNumber` from the host, and `detect` applies that
verdict before classifying the runtime. A host below build 17763 is refused, and
so is a host whose build number cannot be read, so no CEF payload opens under
the declared floor. The terminals are unaffected in both cases.

What the waiver does not buy: nothing on 1809 has been observed. The CEF pin,
the bootstrap sandbox, the D3D presentation path and the installed journey are
unproven on that build. The refusal path itself is proven by test, not by a
1809 machine.

### Which GPU drove the presentation

The application chooses the adapter and the browser follows it. GPUI takes the
first DXGI adapter that satisfies its feature level, which is the one Windows
assigned to `paneflow.exe`, and that device draws every pane, so the browser is
in no position to move it. The application reads the LUID of that device from
its external surface context and passes it to the host in
`PANEFLOW_BROWSER_ADAPTER_LUID`. The host opens exactly that adapter through
`IDXGIFactory4::EnumAdapterByLuid` for its own D3D11 device, and hands the same
LUID to Chromium as `--use-adapter-luid`, which ANGLE turns into the
`EGL_PLATFORM_ANGLE_D3D_LUID_HIGH_ANGLE` and `EGL_PLATFORM_ANGLE_D3D_LUID_LOW_ANGLE`
attributes of the D3D11 display the GPU process creates. The CEF GPU process,
the host and the renderer therefore hold devices on one adapter, which is what a
keyed-mutex D3D11 shared texture requires: it cannot be opened from a device on
another adapter, and the attempt returns `0x80070057`.

The host emits one `paneflow-browser-adapter` line on its standard error at
device creation, carrying the description, the vendor and device identifiers,
the dedicated video memory, the adapter LUID and whether the pin was applied.
That line is what turns a GPU claim into a verifiable one.

Both remaining failure paths stay legible instead of silent. When the host
cannot open the application's adapter, the page never starts and the reason
names the adapter it wanted and the adapters the host can see. When Chromium
ignores the switch and offers a texture from another adapter, the host reports
the adapter that owns that texture, read back with
`IDXGIFactory2::GetSharedResourceAdapterLuid`, beside the import error.

Measured on the reference machine, a Ryzen 7 7800X3D carrying an AMD Radeon
integrated adapter (`0x1002:0x164e`, LUID `0-16920`) and an NVIDIA GeForce RTX
4070 Ti SUPER (`0x10de:0x2705`, LUID `0-1474d`), with the preference set per
executable image through `UserGpuPreferences`:

| Application image | Staged host image | Presentation ran on | Frames | First frame | Log |
|---|---|---|---|---|---|
| integrated, forced | unset, resolved to discrete | nothing, `0x80070057` | 0 | none | `target/ep004-gpu-amd` |
| integrated, forced | unset, resolved to discrete | integrated, pinned | 277 | 2787 ms | `target/ep004-gpu-split-pin` |
| integrated, forced | discrete, forced | integrated, pinned | 276 | 2793 ms | `target/ep004-gpu-split-forced` |
| unset, resolved to discrete | unset | discrete, pinned | 272 | 2859 ms | `target/ep004-gpu-default-pin` |

The first row is the defect, observed on 2026-09-11 before the pin existed. The
third row is the adversarial case: the two images carry opposite explicit
preferences and the presentation still runs on the application's adapter. The
two single-adapter runs that first recorded an adapter identity,
`target/ep004-gpu-nvidia2` and `target/ep004-gpu-amd-run`, each reached 167
frame events and a clean exit before the pin existed and still stand for that
claim.

What the pin does not prove: a switchable-graphics laptop. The reference machine
is a desktop whose two adapters are both enumerable from either process, and a
muxless or Optimus laptop can expose the pair differently. No hardware here
reproduces an application adapter the host cannot open either, so that refusal
is covered by code, not by a run.

The user-facing behavior this target implements is described in
[windows-usage.md](../browser/windows-usage.md).

### Foreground ownership is part of the protocol

An M1 capture is only readable if the benchmark window held the foreground for
the whole capture. Windows throttles the presentation of a background window,
and the terminal then waits for the next frame of a slowed redraw loop, so the
metric that moves is exactly the one M1 exists to measure.

The effect is large and was measured directly. A capture that keeps the
foreground presents about 445 frames per two-second slice; stealing the
foreground with a maximized Notepad twelve seconds into a capture drops it to 74
per slice for the remainder, with nothing else changed:

```
445, 445, 427, 74, 74, 74, 74, 74, 74, 74
```

Across separate captures the split is just as clean: 550 presents and 56.0 ms
`terminal_input_to_present` p95 when the window was backgrounded for 47 of 69
polled samples, against 3252 presents and 35.6 ms when it held the foreground
throughout. The latency decomposes the same way every time: scene to displayed
stays at 14.0 to 14.7 ms p95 whatever happens, and only input to scene moves.

Three changes now keep this out of the numbers:

- `windows-capture.ps1` samples the foreground window every second and records
  `foreground_pid` and `foreground_owned` in `resources.jsonl`.
- `windows-analysis.mjs` reports `window_foreground` and sets the capture status
  to `INVALID_WINDOW_NOT_FOREGROUND` when ownership is not complete. A capture
  recorded before this change reports a null ratio and claims nothing.
- `windows-m1-compare.mjs` refuses a campaign that contains a capture its own
  analysis rejected, rather than averaging it in.

When running a campaign, leave the machine alone: do not click into another
window, and expect notifications to invalidate a repetition.

## Qualification still required

EP-002 executed the first browser page and the D3D external-surface path, and
EP-003 prepared the remaining OS integrations and the package. EP-004 owns the
qualification itself. The following evidence has to be collected on real
hardware before this target changes to a qualified availability, and each item
maps to a row of the contract above:

- A Windows 11 x64 bootstrap and sandbox harness result on a frozen reference
  build. Windows 10 1809 is covered by the owner waiver described above.
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
- A resolution good enough to settle NFR-02 at 120 Hz (US-014). Both campaigns
  were re-run under the corrected harness as `target/ep004-m1-120hz-v2` and
  `target/ep004-m1-60hz-v2`, thirty captures, every one holding the foreground
  for its whole duration. NFR-03 is within budget at both rates. NFR-02 is
  within budget at 60 Hz and 0.106 ms over its 1 ms p95 budget at 120 Hz, on a
  delta whose p99 is negative: the two configurations measure 42.90 and
  44.00 ms, and medians of per-repetition percentiles do not resolve a
  difference that small. Paired per-repetition deltas or more repetitions would
  settle it; a correction designed on this figure would not.
- The rest of the M1 budget evaluation (US-014): GPU memory sampling and the
  separation of interop allocations from internal Chromium VRAM; the C over B
  integration delta NFR-07 needs, which the comparison tool does not compute;
  and any campaign at all for NFR-01, NFR-04, NFR-05, NFR-06, NFR-08 and
  NFR-11.
- A hybrid-GPU run on a switchable-graphics laptop (US-013). The adapter pin
  described above is proven on the reference desktop, including with opposite
  explicit preferences per image, but no muxless or Optimus machine has run it.
- A native Windows agent session covering SEC-09 to SEC-12, the C3 quotas at
  their limit and limit plus one, two workspaces, two clients, stale
  generations, a host restart and a human takeover (US-016).

The accessibility bridge, the permission and file paths, the bundle plan, the
update plan and the agent diagnostics and navigation completion are proven by
contract tests reached from their real entry points on a development host. None
of them claims a native Windows observation.
